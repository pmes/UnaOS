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
//! Mouse: press a slider track to set it and keep the button down to DRAG it (PREFSUI, R93: the press captures
//! the pointer through `video::capture`; every motion sample moves the knob and applies live; the release commits
//! and stores); a toggle/segment/button to act. Keyboard: Up/Down/Tab move the selection,
//! Left/Right adjust, Enter toggles/applies; typing edits the wallpaper path while it is selected.
//!
//! SETTINGS2: TABS (General · Users · Display · About · …; a click or the keys switch, the choice persists as
//! `system.settings.tab`). SETTINGSTABS (rmbp-ledger B623, R118): the tabs are a LEFT COLUMN ([`COL_W`] wide, one
//! [`COL_ROW`] row per tab, scrolling past [`COL_VIS`] rows); every pane draws from ONE origin, [`PANE_X`] x
//! [`PANE_Y`], under a title band carrying the tab's name. Column focus: Up/Down move the tab; Tab moves focus
//! between the column and the pane. Wire: `[settings] tabs=left col_w=<n> rows=<n> sel=<name>` once per open. Users = list + Add / Delete (two-step) / Reset password, every action a
//! `[settings] users op= name= ok= reason=` line; Display = brightness (BRIGHTFLOOR M5), idle blank, UI scale (read-only: the compositor fixes it at
//! takeover), low battery shutdown (SETTINGSMAP: the row the fixed-clock caption held); About = version, board, CPUs, RAM, uptime.
//!
//! SETTINGSCRAFT (rmbp-ledger B630): the window is drawn with GLASSKIT's kit (`video::draw`), DESIGN-LANGUAGE §3.4's
//! Settings: a sidebar (the user card, a search field that filters the tab names, one 28-px pill per tab with a drawn
//! icon tile), and panes whose rows sit in inset GROUP BOXES (hairline, rounded, separators inset from the left, the
//! label left and the control right) with every control one of the kit's (toggle, checkbox, slider, push button,
//! segmented, text field, popup). Hover is polled on the desktop service pass ([`hover_poll`], one repaint per control
//! crossed), a press shows its pressed face for [`PRESS_MS`], the wheel scrolls the column and pages the File Types
//! pane. Every colour is a theme role or derived from one; `tests settings` paints both sets offscreen and checks the
//! pixels (`craft=ok`).
//!
//! A11YPANE (rmbp-ledger B655): the Accessibility tab (the last) — Reduce motion, Increase contrast, Cursor size, Text
//! size — the four `system.accessibility.*` keys, applied as atomics the glass reads (`motion::reduced`,
//! `cursor::block_scale`), loaded at login and followed on PrefChanged; `tests a11y`.
//!
//! SETTINGSMAP (rmbp-ledger B673, BP-bar-17, R118): ONE table, [`MAP`], names every Settings row — the Principia keys it
//! edits, its tab and row, its label — and every `prefs::key::ALL` key is on exactly one row (`system.settings.tab`'s
//! row is the tab column itself). The search field at the column's top reads it: typing filters the column to the tabs
//! whose name or a row's label holds the text, and Return opens [`search_best`]'s hit — the row's tab with its control
//! selected and the row ringed in the accent. The launcher's [`row_of_key`] reads the same table. `tests settingsmap`.
//!
//! REASK (rmbp-ledger B681, R130 law 2): the Privacy tab (the last) — one row per decision the user made (an answered ask
//! kept in Principia's store, `prefs::decisions`), each with a Revoke; the revoke is latched and run on [`service`]
//! (`prefs::revoke`): `[settings] privacy revoke key=<k> ok=<0|1>`. The next time that ask arises, it asks.
//!
//! SMARTLINE (rmbp-ledger B708, BP-bar-1): the Storage tab (the last) — one card per disk, read-only: the model, size,
//! firmware, TRIM; power-on hours, wear, temperature, power cycles; `Health: OK|WARN` with the attribute that tripped.
//! It reads `drivers::ahci::smart`'s boot snapshot (`try_lock`, never a disk command from the render task).
//!
//! Witness: `:: SETTINGS: controls=<n> tabs=<n> loaded=<n> saved=<n> -> PASS ::` on open and on save, and the
//! `tests` fixture `settings` ([`selftest`]: set idle to 5, save, re-read the file, compare).

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::video::draw::{self, Ink, Rect};
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

// SETTINGSTABS (B623, R118) -> SETTINGSCRAFT (B630): the tab COLUMN on the left (the user card, the search field, one
// row per tab with its icon tile); every pane's geometry hangs off ONE origin (PANE_X, PANE_Y). All values are LOGICAL
// px: the surface is `metrics::size` of them (UIMETRICS B372), so the column scales with the rest. The numbers are
// DESIGN-LANGUAGE §3.4's Settings on its 4 pt grid: sidebar 220, pills 28 tall inset 8, a 16 icon at 8 with the label
// 8 after it, pane padding 20, grouped-list rows 48 (the language's minimum is 40) with an inset of 12.
/// The tab column's width (logical px).
pub const COL_W: usize = 220;
/// One tab row's pitch in the column (the pill is [`PILL_H`] tall, centred in it).
const COL_ROW: usize = 32;
/// The user card at the column's top: the avatar disc, the name, the role (a press opens Users).
const CARD_Y: usize = 12;
const AVATAR: usize = 32;
const CARD_H: usize = AVATAR + 8;
/// The search field under the card: it filters the tab names.
const SEARCH_X: usize = 8;
const SEARCH_Y: usize = CARD_Y + CARD_H + 8;
const SEARCH_W: usize = COL_W - 2 * SEARCH_X;
const SEARCH_H: usize = draw::DIMS.field_h;
/// The first tab row's top.
const COL_PAD: usize = SEARCH_Y + SEARCH_H + 12;
/// The pill's inset and height, the icon tile and the label after it.
const PILL_X: usize = 8;
const PILL_H: usize = draw::DIMS.row_h;
const ICON: usize = 16;
const ICON_X: usize = PILL_X + 8;
const COL_LABEL_X: usize = ICON_X + ICON + 8;
/// The pane's content origin: x right of the column, y below the pane's title band (the tab's name as a heading).
const PANE_X: usize = COL_W;
const PANE_Y: usize = 60;
/// The pane's own width (SETTINGSCRAFT: the window is 920 x 560, the proportion the language's Settings keeps).
const PANE_W: usize = 700;
const WIN_W: usize = PANE_X + PANE_W;
/// The first control row's top: the pane origin (the title band is above it).
const TOP: usize = PANE_Y;
const ROW_H: usize = 48;
const ROWS: usize = 10;
/// The tabs, in the column's order.
pub const TABS: usize = 16;
pub(super) const TAB_NAMES: [&str; TABS] = ["General", "Users", "Display", "About", "Login Items", "Appearance", "Trackpad", "File Types", "Notifications", "Accessibility", "Privacy", "Date & Time", "Keyboard", "Storage", "Security", "Hardware"]; // PREFSUI (R91) · APPEARANCE (B408) 5 · TRACKPADPANE · TZLOCALE (B701) 11 · KEYMAPS (B704) 12 · SMARTLINE (B708) 13 · AUTOLOCK (B712) 14 · HWTAB (B737) 15
const WIN_H: usize = TOP + ROWS * ROW_H + PAD;
/// Tab rows the column shows at once; more than this and it scrolls (the wheel, or the selection kept in view).
const COL_VIS: usize = (WIN_H - COL_PAD - 8) / COL_ROW;
/// The pane's padding, a group box's inset from its band of rows, a row's inset inside its box.
const PAD: usize = 20;
const GAP: usize = 4;
const INSET: usize = 12;
const BOX_X: usize = PANE_X + PAD;
const BOX_W: usize = PANE_W - 2 * PAD;
const LABEL_X: usize = BOX_X + INSET;
/// The right edge every row's control is aligned to.
const CTRL_R: usize = BOX_X + BOX_W - INSET;
const TRACK_X: usize = PANE_X + 300;
const TRACK_W: usize = 240;
const VAL_X: usize = TRACK_X + TRACK_W + 12;
const BTN_W: usize = 70;
const BTN_H: usize = draw::DIMS.button_h;
const FIELD_H: usize = draw::DIMS.field_h;
/// A row's switch, right-aligned.
const TOG_X: usize = CTRL_R - draw::DIMS.toggle_w;

static WIN: AtomicU32 = AtomicU32::new(wm::WIN_NONE);
static LOADED_N: AtomicU32 = AtomicU32::new(0);
static SAVED_N: AtomicU32 = AtomicU32::new(0);
static OPEN_REQ: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static STATE: crate::sync::Mutex<Option<State>> = crate::sync::Mutex::new(None);
/// The session user the file was last loaded for (empty = never loaded).
static LOADED_FOR: crate::sync::Mutex<String> = crate::sync::Mutex::new(String::new()); static SESSION_SEEN: crate::prefs::SessionSeen = crate::prefs::SessionSeen::new(); // SESSIONGEN (B462)
static CUR: crate::sync::Mutex<Values> = crate::sync::Mutex::new(Values::DEFAULT);

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
    /// Focus is on the tab column (Up/Down move the tab; Tab moves focus to the pane).
    strip: bool,
    /// SETTINGSTABS: the column's first visible tab row (it scrolls past `COL_VIS`) and the hovered row.
    col_top: usize,
    col_hover: Option<usize>,
    u: UsersUi,
    /// PREFSUI: the Login Items tab's Add chooser (an index into `loginitems::installed()`), and the Display
    /// tab's Resolution dropdown (open or not).
    li_add: usize,
    modes_open: bool,
    w: usize,
    h: usize,
    surf: Vec<u32>,
    /// SETTINGSCRAFT: the column's search text and whether the field holds the keys.
    query: String,
    search: bool,
    /// The last selection move came from the keyboard (the focus ring shows only then: DESIGN-LANGUAGE §2.7).
    kbd: bool,
    /// The pointer over the window (logical px), a press being shown pressed `(x, y, at ms)`, the controls' rects
    /// this paint drew (logical `x, y, w, h`) and which of them is under the pointer.
    ptr: Option<(usize, usize)>,
    press: Option<(usize, usize, u64)>,
    hits: Vec<[u16; 4]>,
    hot: Option<usize>,
    /// MOTIONKIT (B648): the values in motion (the pill's y, each toggle's knob, a segmented thumb's x — keyed by
    /// [`mkey`]), the frame gate [`hover_poll`] asks, and this paint's clock and tab (read once at the top of
    /// [`paint`]: a control helper never locks `CUR`, which [`open`] holds across its first paint).
    glides: draw::motion::Glides,
    frame: draw::motion::Frame,
    now: u64,
    tab: usize,
    /// SETTINGSMAP (B673): the `(tab, row)` the search's Return opened, ringed until the next press, key or tab switch.
    found: Option<(u8, u8)>,
}

impl State {
    fn new(w: usize, h: usize, surf: Vec<u32>, sel: usize, strip: bool, col_top: usize) -> State {
        State {
            sel, strip, col_top, col_hover: None, u: UsersUi::new(), li_add: 0, modes_open: false, w, h, surf,
            query: String::new(), search: false, kbd: false, ptr: None, press: None, hits: Vec::new(), hot: None,
            glides: draw::motion::Glides::new(), frame: draw::motion::Frame::new(), now: 0, tab: 0, found: None,
        }
    }
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
    // BRIGHTSLIDER M3 (B377, R89): the stored level is NOT written at login — the slider seeds from the panel's own
    // register (`[backlight] login keep readback= … wrote=0`); the store records the user's moves, it does not override the glass.
    let kept = crate::video::backlight::login_keep(if mask & 1 != 0 { Some(v.bright) } else { None });
    CUR.lock().bright = kept;
    if mask & 2 != 0 || mask & 4 != 0 { apply_volume(v.vol, v.mute); }
    if mask & 8 != 0 { apply_idle(v.idle_min); }
    tp_load(if mask & 16 != 0 { Some(v.ptr) } else { None }); // TRACKPADPANE (B412): the five trackpad keys; a legacy pointer.speed stands in for an unset speed
    if mask & 32 != 0 && !v.wall.is_empty() { apply_wall(&v.wall); }
    mode_at_login(); super::appearance::load_at_login(); a11y_load("login"); kbd_load("login"); // KEYMAPS (B704): the layout and the Compose switch, live for the decoders' seam. A11YPANE (B655): the four accessibility keys, latched for the glass. PREFSUI (R93): a stored `system.display.mode` is this session's scale
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
    { let _ = crate::video::crystal::login::open_change_password(n.as_bytes()); say("password", "set-password-screen", true); } // SETTINGSREKEY (B483): the owner's change asks the old password and re-keys the ring
    #[cfg(not(feature = "login"))]
    { let _ = n; say("password", "login-feature-off", false); }
}

/// Adjust control `i` by `d` steps (Left/Right).
fn adjust(i: usize, d: isize) {
    let c = CUR.lock().clone();
    let step = |cur: usize, max: usize| -> usize { (cur as isize + d).clamp(0, max as isize) as usize };
    match i {
        0 => bright_key(d > 0),
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
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    { let _s = super::loginrender::stamp(super::loginrender::W_S_LAUNCHER); super::launcher::service(); #[cfg(all(target_arch = "x86_64", feature = "wc"))] super::appswitch::service(); } /* APPSWITCH (B428): the switcher's pass. KVMLAT2 (B579): each part of this fn is a nested `loginrender` stamp */ // LAUNCHER (B417): the launcher's pass (snapshot, ranking, picks, the recency file) — off the input router
    #[cfg(all(target_arch = "aarch64", feature = "login"))] super::lock::service(); /* crawl A 14: AUTOLOCK's pass on aarch64 desktop_firmware (x86 runs it from `login::submit_drain`) */ let av = { let _s = super::loginrender::stamp(super::loginrender::W_S_ASSOC); crate::fs::assoc::view_service() }; if av && is_open() && cur_tab() == FT_TAB { repaint(); } // FILETYPES (B423): the registry's rows / a preferred-app change, applied off the click path
    #[cfg(any(target_arch = "aarch64", feature = "unafs"))] { let _s = super::loginrender::stamp(super::loginrender::W_S_NAMEINDEX); crate::fs::nameindex::service(); } // NAMEINDEX (B432): the login task's chunked name-index build (idle: one load)
    { let _s = super::loginrender::stamp(super::loginrender::W_S_PREFS); crate::prefs::service(); } { let _s = super::loginrender::stamp(super::loginrender::W_S_LOGINITEMS); super::loginitems::service(); } { let _s = super::loginrender::stamp(super::loginrender::W_S_FILES); super::settingsfiles::service(); } { let _s = super::loginrender::stamp(super::loginrender::W_S_APPEARANCE); super::appearance::service(); super::look::service(); } // SOFTY (B629): the look's pass (the login's load, PrefChanged, the Settings choice, the Start menu's hover) rides the appearance stamp. PREFSUI (R91): the session's login items — read, saved, launched after the desktop
    // BRIGHTFLOOR M2: the level loaded at the previous pass's login is applied HERE, one pass later.
    if super::notifypane::take_stale() && is_open() && cur_tab() == NP_TAB { repaint(); } // NOTIFYPANE (B435): a newly seen app joins the open pane
    let la = LOGIN_APPLY.swap(0, Ordering::AcqRel);
    if la != 0 { let on = apply_bright_via(la, "prefchanged"); say("brightness", &alloc::format!("{}", la), on); if is_open() { repaint(); } } // BRIGHTSLIDER M3: only another client's PrefSet lands here now (the login writes nothing)
    let mut wb = [0u8; crate::prefs::WHO_BUF]; let (sk, sq) = SESSION_SEEN.check(); let who = if sq.is_none() { let _s = super::loginrender::stamp(super::loginrender::W_S_SESSION); crate::prefs::user_name_in(&mut wb) } else { None }; let has = sq.unwrap_or(who.is_some()); if sq.is_none() { SESSION_SEEN.mark(sk, has); } // SESSIONGEN (B462): the session is compared only when the login generation moved; PERFREVIEW F3 (B443): the per-pass session reads go on the stack
    if has {
        let fresh = match who { Some(u) => { let mut g = LOADED_FOR.lock(); if *g != u { *g = String::from(u); true } else { false } } None => false }; // SESSIONGEN: quiet => this generation was compared (and recorded) already
        // BRIGHTFLOOR M5: the session opened — the load (and the safe-mode Shift check) waits for the
        // first desktop pass SAFE_SETTLE_MS later, so a Shift still down from the password's last
        // character at Enter is never read as the reset.
        if fresh { LOAD_DONE.store(false, Ordering::Release); LOGIN_AT.store(crate::arch::ms().max(1), Ordering::Release); }
        let at = LOGIN_AT.load(Ordering::Acquire);
        if at != 0 && crate::arch::ms().saturating_sub(at) >= SAFE_SETTLE_MS {
            LOGIN_AT.store(0, Ordering::Release);
            { let _s = super::loginrender::stamp(super::loginrender::W_S_LOAD); load_for_login(); } // KVMLAT2 (B579)
            LOAD_DONE.store(true, Ordering::Release);
        }
    }
    // The brightness keys (F1/F2) and the volume keys (F10-F12) change the live level from the input
    // paths, where no VFS work may run: this pass notices the change and persists it (PREFS B300).
    // BRIGHTFLOOR M5: only once the login's load has run (a key before it must not overwrite the store).
    if has && LOAD_DONE.load(Ordering::Acquire) { // SESSIONGEN (B462): `has` => LOADED_FOR holds this session's (non-empty) name
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
        if db { let _s = super::loginrender::stamp(super::loginrender::W_S_PERSIST); persist(0); }
        if dv { let _s = super::loginrender::stamp(super::loginrender::W_S_PERSIST); persist(1); }
        let pa = BRIGHT_PERSIST_AT.load(Ordering::Acquire); // BRIGHTSLIDER M2: the slider's store write, debounced off the press
        if pa != 0 && crate::arch::ms() >= pa && BRIGHT_PERSIST_AT.compare_exchange(pa, 0, Ordering::AcqRel, Ordering::Relaxed).is_ok() { let _s = super::loginrender::stamp(super::loginrender::W_S_PERSIST); persist(0); }
    }
    { let _s = super::loginrender::stamp(super::loginrender::W_S_MODE); mode_service(); tp_service(); bus_changes(); a11y_service(); lowbat_service(); privacy_service(); kbd_service(); security_service(); hw_service(); #[cfg(all(target_arch = "x86_64", feature = "wc", feature = "unafs"))] backup::service(); #[cfg(all(target_arch = "x86_64", feature = "installdemo"))] sysimg::service(); if crate::clock::locale::service() && is_open() && cur_tab() == DT_TAB { repaint(); } } if OPEN_REQ.swap(false, Ordering::AcqRel) { // PREFSUI: a chosen mode is applied here, off the click router
        if let Err(e) = open() {
            serial_println!("[settings] refuse reason={}", e);
        }
    }
}

// ── Tabs: which controls live where, and the layout helpers ──────────────────────────────────

/// Control indices on `tab`, in keyboard order (BRIGHTFLOOR M5: General = volume, mute, pointer,
/// wallpaper, apply, off, password; Display = brightness, idle blank; Users/About: none).
fn tab_ctrls(tab: usize) -> &'static [usize] {
    match tab { 0 => &[1, 2, 5, 6, 7, 8], 2 => &[0, 3, 9, 10], _ => &[] } // KERNELFONT2: 9 font, 10 font size
}

/// Row of control `i` on its tab (General: 1→0, 2→1, 4→2, 5→3, 6|7→4, 8→5; Display: 0→0, 3→1).
const fn row_of(i: usize) -> usize {
    match i { 0 => 0, 1 => 0, 2 => 1, 3 => 1, 4 => 2, 5 => 3, 6 | 7 => 4, 9 => 4, 10 => 5, _ => 5 }
}

// ── SETTINGSCRAFT (rmbp-ledger B630) — the window drawn with the kit (`video::draw`, GLASSKIT B628) ──────────────────
// Every control below is one of the kit's: no bare `fill_rect` box, no 1-px border made of fills. A control's state
// comes from [`cs`], which also records its logical rect in `State::hits`, so the hover pass ([`hover_route`]) knows
// when the pointer crosses into another control: one repaint per crossing, none per motion sample.

/// How long a press shows its pressed face (DESIGN-LANGUAGE §2.6 `motion.fast`).
pub const PRESS_MS: u64 = 100;
/// The column's ground: the window's face this far (of 255) toward the ink — darker on Light, lighter on Dark, as
/// DESIGN-LANGUAGE §2.3's `surface.sidebar` sits against `surface.window` in each set. Derived, never a literal.
const SIDEBAR_TINT: u32 = 12;

/// A logical coordinate's device edge (the kit draws in device px).
fn e(v: usize) -> i32 {
    super::metrics::edge(v) as i32
}

/// A logical span's device width.
fn ew(x: usize, w: usize) -> i32 {
    e(x + w) - e(x)
}

/// A logical length at the panel's scale (device px), as the kit sizes its controls.
fn dp(n: usize) -> i32 {
    crate::ui::px(n) as i32
}

/// Row `r`'s logical top.
const fn row_top(r: usize) -> usize {
    TOP + r * ROW_H
}

/// The top of an `h`-tall control centred in row `r`.
const fn mid_y(r: usize, h: usize) -> usize {
    row_top(r) + (ROW_H - h) / 2
}

/// The kit's surface over the window's pixels.
fn sf(st: &mut State) -> draw::Surf<'_> {
    let pw = super::metrics::size(st.w);
    draw::Surf::new(&mut st.surf, pw)
}

fn sidebar_bg() -> u32 {
    let _ = SIDEBAR_TINT; theme::surface_sidebar() // DESIGNAPPLY (B639): `surface.sidebar` (§2.3), the token for this job (was the face 12/255 toward the ink)
}

/// A control's state at the logical rect `(x, y, w, h)` (hover / pressed from the pointer); records the rect.
fn cs(st: &mut State, x: usize, y: usize, w: usize, h: usize, focused: bool) -> draw::ControlState {
    st.hits.push([x as u16, y as u16, w as u16, h as u16]);
    let inside = |p: Option<(usize, usize)>| p.is_some_and(|(px, py)| px >= x && px < x + w && py >= y && py < y + h);
    draw::ControlState { hover: inside(st.ptr), pressed: inside(st.press.map(|(a, b, _)| (a, b))), disabled: false, focused }
}

/// The topmost recorded control under `p`.
fn hit_at(hits: &[[u16; 4]], p: Option<(usize, usize)>) -> Option<usize> {
    let (px, py) = p?;
    hits.iter().rposition(|h| {
        let (x, y, w, hh) = (h[0] as usize, h[1] as usize, h[2] as usize, h[3] as usize);
        px >= x && px < x + w && py >= y && py < y + hh
    })
}

/// Text with its line top at the logical `(x, y)`, clipped at the logical `clip`; returns the pen.
#[allow(clippy::too_many_arguments)]
fn text_at(st: &mut State, x: usize, y: usize, clip: usize, t: &str, ink: u32, bold: bool, face: super::text::Face) -> usize {
    let (w, h) = (st.w, st.h);
    super::metrics::text(&mut st.surf, super::metrics::size(w), super::metrics::size(h), clip, x, y, t.as_bytes(), ink, bold, face)
}

/// A row's text in `ink`, vertically centred in row `r`, clipped at the controls' right edge.
fn txt_ink(st: &mut State, x: usize, r: usize, t: &str, ink: u32) {
    let face = super::text::Face::Ui; // KERNELFONT: labels in the UI face
    let ch = super::metrics::lcell_h(face);
    text_at(st, x, row_top(r) + (ROW_H - ch) / 2, CTRL_R.max(x + 1), t, ink, false, face);
}

/// A row's label (the primary ink).
fn txt(st: &mut State, x: usize, r: usize, t: &str) {
    txt_ink(st, x, r, t, theme::text_primary());
}

/// A row's value, a section header or help text (the secondary ink).
fn txt2(st: &mut State, x: usize, r: usize, t: &str) {
    txt_ink(st, x, r, t, theme::ink_secondary());
}

/// The kit's push button `w` wide at the logical `(x, y)`.
#[allow(clippy::too_many_arguments)]
fn btn_at(st: &mut State, x: usize, y: usize, w: usize, t: &str, default: bool, focus: bool) {
    let c = cs(st, x, y, w, BTN_H, focus);
    draw::push_button(&mut sf(st), e(x), e(y), ew(x, w), t.as_bytes(), default, c);
}

/// A [`BTN_W`] push button in row `r` at `x`.
fn btn(st: &mut State, r: usize, x: usize, t: &str) {
    btn_at(st, x, mid_y(r, BTN_H), BTN_W, t, false, false);
}

/// [`btn`] that may be the pane's default action and may hold the keyboard focus.
fn btn_f(st: &mut State, r: usize, x: usize, t: &str, default: bool, focus: bool) {
    btn_at(st, x, mid_y(r, BTN_H), BTN_W, t, default, focus);
}

/// The kit's text field in row `r` from the track to the controls' edge; focused, it carries the caret and the ring.
fn field(st: &mut State, r: usize, t: &str, focus: bool) {
    let (y, w) = (mid_y(r, FIELD_H), CTRL_R - TRACK_X);
    let c = cs(st, TRACK_X, y, w, FIELD_H, focus);
    draw::text_field(&mut sf(st), e(TRACK_X), e(y), ew(TRACK_X, w), t.as_bytes(), c);
}

/// The kit's slider over the track in row `r`. The knob's centre is the press map's point (`slider_at`): the drawn
/// rail is the track grown by half a knob at each end.
fn slider(st: &mut State, r: usize, pos: usize, max: usize, focus: bool) {
    let kd = draw::DIMS.slider_knob;
    let y = mid_y(r, kd);
    let c = cs(st, TRACK_X, y, TRACK_W, kd, focus);
    let k = dp(kd);
    draw::slider(&mut sf(st), e(TRACK_X) - k / 2, e(y), ew(TRACK_X, TRACK_W) + k, pos as u32, max as u32, c);
}

// ── MOTIONKIT (rmbp-ledger B648) — the three motions (DESIGN-LANGUAGE §2.6, `motion.base` 160 ms / `ease.standard`) ──
// The sidebar pill eases between rows, a toggle's knob slides, a segmented thumb slides. Each is a keyed value in
// `State::glides`: the control helper asks for the value to draw THIS frame given its target, the glides start a tween
// when the target moved, and `hover_poll` (the desktop service pass) repaints once per 16 ms frame while one runs and
// never while idle (`State::frame` counts the frames it granted). Reduce motion (`motion::reduced()`) makes each a cut.

/// A motion key: `kind` (1 pill, 2 toggle, 3 segmented) over the tab, the row and a logical x (a toggle's).
const fn mkey(kind: u32, tab: usize, r: usize, x: usize) -> u32 {
    (kind << 28) | ((tab as u32 & 0xff) << 20) | ((r as u32 & 0xff) << 12) | (x as u32 & 0xfff)
}
const MK_PILL: u32 = 1;
const MK_TOGGLE: u32 = 2;
const MK_SEG: u32 = 3;

/// The kit's switch in row `r` at `x`: its knob glides to `on`'s end.
fn tog(st: &mut State, r: usize, x: usize, on: bool, focus: bool) {
    let (tw, th) = (draw::DIMS.toggle_w, draw::DIMS.toggle_h);
    let y = mid_y(r, th);
    let c = cs(st, x, y, tw, th, focus);
    let (now, tab) = (st.now, st.tab);
    let knob = st.glides.to(mkey(MK_TOGGLE, tab, r, x), if on { draw::motion::ONE as i32 } else { 0 }, now, draw::motion::dur(draw::motion::BASE), draw::motion::Curve::Standard);
    draw::toggle_at(&mut sf(st), e(x), e(y), knob as u32, c);
}

/// The kit's check box in row `r` at `x`.
fn check(st: &mut State, r: usize, x: usize, on: bool) {
    let b = draw::DIMS.check;
    let y = mid_y(r, b);
    let c = cs(st, x, y, b, b, false);
    draw::checkbox(&mut sf(st), e(x), e(y), on, c);
}

/// The kit's segmented control over the track in row `r` (equal segments: the press map is `x / (TRACK_W / n)`);
/// its thumb glides to segment `sel`.
fn seg(st: &mut State, r: usize, labels: &[&[u8]], sel: usize, focus: bool) {
    let h = draw::DIMS.seg_h;
    let y = mid_y(r, h);
    let c = cs(st, TRACK_X, y, TRACK_W, h, focus);
    let (x0, w) = (e(TRACK_X), ew(TRACK_X, TRACK_W));
    let target = x0 + (w / labels.len().max(1) as i32) * sel.min(labels.len().saturating_sub(1)) as i32;
    let (now, tab) = (st.now, st.tab);
    let tx = st.glides.to(mkey(MK_SEG, tab, r, 0), target, now, draw::motion::dur(draw::motion::BASE), draw::motion::Curve::Standard);
    draw::segmented_at(&mut sf(st), x0, e(y), w, labels, sel, tx, c);
}

/// The inset grouped boxes of a pane: one per `(first row, last row)`, a hairline between its rows inset from the
/// left (DESIGN-LANGUAGE §3.4's grouped list). Drawn first; the rows' text and controls go on top.
fn groups(st: &mut State, g: &[(usize, usize)]) {
    let mut s = sf(st);
    for &(a, b) in g {
        let (y0, y1) = (row_top(a) + GAP, row_top(b + 1) - GAP);
        let r = Rect::new(e(BOX_X), e(y0), ew(BOX_X, BOX_W), e(y1) - e(y0));
        draw::group_box(&mut s, r, b"");
        let x = e(BOX_X + INSET);
        for k in a + 1..=b {
            draw::hairline_h(&mut s, x, e(row_top(k)), r.right() - 1 - x, Ink::argb(theme::hairline()));
        }
    }
}

/// A wash over row `r` inside its box (a list's selected or hovered row).
fn row_wash(st: &mut State, r: usize) {
    let (x, y) = (BOX_X + GAP, row_top(r) + GAP);
    let rr = Rect::new(e(x), e(y), ew(x, BOX_W - 2 * GAP), ew(y, ROW_H - 2 * GAP));
    draw::fill_rrect(&mut sf(st), rr, dp(draw::DIMS.row_radius), Ink::argb(theme::hairline()));
}

/// An avatar disc `d` across at the logical `(x, y)` with the name's initial (the user card, the Users list).
fn avatar(st: &mut State, x: usize, y: usize, d: usize, name: &str) {
    draw::fill_disc(&mut sf(st), Rect::new(e(x), e(y), ew(x, d), ew(y, d)), theme::accent());
    let one = [name.bytes().find(|b| b.is_ascii_alphanumeric()).map(|b| b.to_ascii_uppercase()).unwrap_or(b'?')];
    let face = super::text::Face::Ui;
    let (iw, ih) = (super::metrics::ladvance(&one, true, face), super::metrics::lcell_h(face));
    text_at(st, x + d.saturating_sub(iw) / 2, y + d.saturating_sub(ih) / 2, x + d, core::str::from_utf8(&one).unwrap_or("?"), theme::ink_on_accent(), true, face);
}

// ── The tab icons: our own glyphs, drawn as code on a rounded tile (DESIGN-LANGUAGE §2.8, §7.3 item 1) ────────────

/// A tab's tile colour: the accents table and the ink — no literal (General, About, Login Items, Trackpad and File
/// Types the slate; Users and Display the first accent; Appearance the ink itself; Notifications the rose;
/// Accessibility, Privacy and Storage the slate).
fn icon_tile(tab: usize) -> u32 {
    match tab {
        1 | 2 => theme::ACCENTS[0],
        5 => theme::text_primary(),
        8 => theme::ACCENTS[5],
        _ => theme::ACCENTS[7],
    }
}

/// Tab `tab`'s icon: a `t`-px rounded tile at the device `(x, y)` (in `tile`, or the tab's own colour) and its glyph
/// in the ink drawn on a fill. Every shape is a kit primitive at a fraction of the tile, so it scales with the panel.
fn tab_icon(s: &mut draw::Surf, tab: usize, x: i32, y: i32, t: i32, tile: Option<u32>) {
    use draw::{fill_circle, fill_rrect, line_aa, stroke_circle, stroke_rrect, Radii, SUB};
    let bg = tile.unwrap_or_else(|| icon_tile(tab));
    fill_rrect(s, Rect::new(x, y, t, t), draw::icon_radius(t as usize) as i32, bg);
    let ink = if tab == 5 && tile.is_none() { theme::surface_content() } else { theme::ink_on_accent() };
    let f = |n: i32| t * n / 100;
    let (cx, cy) = (x + t / 2, y + t / 2);
    let sw = f(10).max(1);
    let saved = s.clip;
    s.set_clip(saved.intersect(Rect::new(x, y, t, t)));
    match tab {
        0 => {
            // General: a gear — a disc, eight teeth off the axes, a hole in the tile's colour.
            const DIRS: [(i32, i32); 8] = [(92, 38), (38, 92), (-38, 92), (-92, 38), (-92, -38), (-38, -92), (38, -92), (92, -38)];
            fill_circle(s, cx, cy, f(27).max(2), ink);
            let (r1, w) = (f(36), f(15).max(1) * SUB);
            for (dx, dy) in DIRS {
                line_aa(s, cx * SUB, cy * SUB, cx * SUB + dx * r1 * SUB / 100, cy * SUB + dy * r1 * SUB / 100, w, ink);
            }
            fill_circle(s, cx, cy, f(11).max(1), bg);
        }
        1 => {
            // Users: a head and shoulders.
            fill_circle(s, cx, y + f(36), f(17).max(1), ink);
            fill_rrect(s, Rect::new(x + f(20), y + f(60), f(60), f(40)), Radii::new(f(28), f(28), 0, 0), ink);
        }
        2 => {
            // Display: a screen on a stand.
            stroke_rrect(s, Rect::new(x + f(16), y + f(20), f(68), f(46)), f(8), sw, ink);
            fill_rrect(s, Rect::new(cx - f(6), y + f(66), f(12).max(1), f(10).max(1)), 0, ink);
            fill_rrect(s, Rect::new(x + f(30), y + f(74), f(40), f(8).max(1)), f(4), ink);
        }
        3 => {
            // About: an "i" — a dot over a stem.
            fill_circle(s, cx, y + f(27), f(9).max(1), ink);
            fill_rrect(s, Rect::new(cx - f(7), y + f(42), f(14).max(2), f(36)), f(7), ink);
        }
        4 => {
            // Login Items: a list — three bullets and three bars.
            for i in 0..3 {
                let yy = y + f(28 + 22 * i);
                fill_circle(s, x + f(28), yy, f(6).max(1), ink);
                fill_rrect(s, Rect::new(x + f(40), yy - f(5), f(38), f(10).max(1)), f(5), ink);
            }
        }
        5 => {
            // Appearance: a half-lit disc — the light half filled, the dark half the tile's, one ring around both.
            let r = f(32).max(2);
            fill_circle(s, cx, cy, r, ink);
            s.set_clip(saved.intersect(Rect::new(cx, y, t / 2 + 1, t)));
            fill_circle(s, cx, cy, r - sw, bg);
            s.set_clip(saved.intersect(Rect::new(x, y, t, t)));
            stroke_circle(s, cx, cy, r, sw, ink);
        }
        6 => {
            // Trackpad: a rounded pad with its click strip.
            let pad = Rect::new(x + f(18), y + f(22), f(64), f(56));
            stroke_rrect(s, pad, f(12), sw, ink);
            fill_rrect(s, Rect::new(pad.x, y + f(62), pad.w, sw), 0, ink);
        }
        7 => {
            // File Types: a page with its corner turned and two lines of text.
            fill_rrect(s, Rect::new(x + f(26), y + f(16), f(48), f(68)), Radii::new(f(6), f(22), f(6), f(6)), ink);
            fill_rrect(s, Rect::new(x + f(34), y + f(46), f(32), sw), 0, bg);
            fill_rrect(s, Rect::new(x + f(34), y + f(60), f(22), sw), 0, bg);
        }
        HW_TAB => { fill_rrect(s, Rect::new(x + f(28), y + f(28), f(44), f(44)), f(6), ink); for i in 0..3i32 { let p = f(34 + 13 * i); fill_rrect(s, Rect::new(x + p, y + f(16), f(6).max(1), f(12)), 0, ink); fill_rrect(s, Rect::new(x + p, y + f(72), f(6).max(1), f(12)), 0, ink); fill_rrect(s, Rect::new(x + f(16), y + p, f(12), f(6).max(1)), 0, ink); fill_rrect(s, Rect::new(x + f(72), y + p, f(12), f(6).max(1)), 0, ink); } fill_rrect(s, Rect::new(x + f(40), y + f(40), f(20), f(20)), f(3), bg); } SEC_TAB => { stroke_rrect(s, Rect::new(x + f(26), y + f(14), f(48), f(70)), f(14), sw, ink); fill_rrect(s, Rect::new(cx - f(5), y + f(30), f(10).max(1), f(26)), f(5), ink); fill_circle(s, cx, y + f(64), f(6).max(1), ink); } /* Hardware (HWTAB B737): a chip — the body, three pins a side, the die in the tile's colour. Security (AUTOLOCK B712): a shield — its outline, a bar and a dot (the lock's mark). LINE-NEUTRAL fold. */ PRIV_TAB => {
            // Privacy: a padlock — the shackle's arch, the body, the keyhole in the tile's colour (REASK B681).
            stroke_rrect(s, Rect::new(x + f(32), y + f(16), f(36), f(44)), f(18), sw, ink);
            fill_rrect(s, Rect::new(x + f(22), y + f(44), f(56), f(40)), f(8), ink);
            fill_circle(s, cx, y + f(62), f(7).max(1), bg);
        }
        DT_TAB => {
            // Date & Time: a clock face — the ring, the hour hand to ten, the minute hand to twelve, the hub (TZLOCALE B701).
            stroke_circle(s, cx, cy, f(36).max(2), sw, ink);
            line_aa(s, cx * SUB, cy * SUB, cx * SUB, (y + f(26)) * SUB, sw * SUB, ink);
            line_aa(s, cx * SUB, cy * SUB, (x + f(32)) * SUB, (y + f(40)) * SUB, sw * SUB, ink);
            fill_circle(s, cx, cy, f(6).max(1), ink);
        }
        KB_TAB => {
            // Keyboard: a rounded deck, two rows of keys and the space bar (KEYMAPS B704).
            stroke_rrect(s, Rect::new(x + f(12), y + f(28), f(76), f(46)), f(8), sw, ink);
            for i in 0..4i32 {
                fill_rrect(s, Rect::new(x + f(22 + 15 * i), y + f(37), f(9).max(1), f(8).max(1)), f(2), ink);
            }
            for i in 0..3i32 {
                fill_rrect(s, Rect::new(x + f(29 + 15 * i), y + f(49), f(9).max(1), f(8).max(1)), f(2), ink);
            }
            fill_rrect(s, Rect::new(x + f(30), y + f(61), f(40), f(6).max(1)), f(3), ink);
        }
        STORAGE_TAB => {
            // Storage: a drive — the body, its activity light and the seam, in the tile's colour (SMARTLINE B708).
            fill_rrect(s, Rect::new(x + f(14), y + f(30), f(72), f(40)), f(8), ink);
            fill_circle(s, x + f(70), y + f(50), f(6).max(1), bg);
            fill_rrect(s, Rect::new(x + f(24), y + f(48), f(30), sw), 0, bg);
        }
        9 => {
            // Accessibility: the universal-access figure — a ring, the head, the arms across, the two legs (A11YPANE B655).
            stroke_circle(s, cx, cy, f(44).max(2), sw, ink);
            fill_circle(s, cx, y + f(30), f(8).max(1), ink);
            fill_rrect(s, Rect::new(x + f(28), y + f(42), f(44), sw), f(2), ink);
            line_aa(s, cx * SUB, (y + f(46)) * SUB, (x + f(36)) * SUB, (y + f(76)) * SUB, sw * SUB, ink);
            line_aa(s, cx * SUB, (y + f(46)) * SUB, (x + f(64)) * SUB, (y + f(76)) * SUB, sw * SUB, ink);
        }
        _ => {
            // Notifications: a bell — the dome, the rim, the clapper, the crown.
            fill_rrect(s, Rect::new(x + f(28), y + f(24), f(44), f(42)), Radii::new(f(22), f(22), f(4), f(4)), ink);
            fill_rrect(s, Rect::new(x + f(20), y + f(60), f(60), f(10).max(1)), f(5), ink);
            fill_circle(s, cx, y + f(76), f(8).max(1), ink);
            fill_circle(s, cx, y + f(22), f(6).max(1), ink);
        }
    }
    s.clip = saved;
}

// ── The column ────────────────────────────────────────────────────────────────────────────────────────────────

/// The column's pill top for list position `i` (with `top` the first visible).
const fn pill_y(i: usize, top: usize) -> usize {
    COL_PAD + (i - top) * COL_ROW + (COL_ROW - PILL_H) / 2
}

fn paint_column(st: &mut State, tab: usize) {
    let h = st.h;
    {
        let mut s = sf(st);
        draw::fill(&mut s, Rect::new(0, 0, e(COL_W), e(h)), sidebar_bg());
        draw::hairline_v(&mut s, e(COL_W) - 1, 0, e(h), Ink::argb(theme::hairline()));
    }
    // The user card: who is signed in and in what role. A press opens Users.
    let who = user_name();
    let (name, role) = match who.as_deref() {
        Some(n) => (String::from(n), if users_is_admin(n) { "Administrator" } else { "Standard user" }),
        None => (String::from("No session"), "Sign in to keep settings"),
    };
    let cw = COL_W - 2 * PILL_X;
    let c = cs(st, PILL_X, CARD_Y, cw, CARD_H, false);
    if c.hover || c.pressed {
        let r = Rect::new(e(PILL_X), e(CARD_Y), ew(PILL_X, cw), ew(CARD_Y, CARD_H));
        draw::fill_rrect(&mut sf(st), r, dp(draw::DIMS.row_radius), Ink::argb(theme::hairline()));
    }
    let (ax, ay) = (PILL_X + 4, CARD_Y + (CARD_H - AVATAR) / 2);
    avatar(st, ax, ay, AVATAR, &name);
    let face = super::text::Face::Ui;
    let ch = super::metrics::lcell_h(face);
    let tx = ax + AVATAR + 10;
    text_at(st, tx, CARD_Y + CARD_H / 2 - ch, COL_W - PILL_X, &name, theme::text_primary(), true, face);
    text_at(st, tx, CARD_Y + CARD_H / 2, COL_W - PILL_X, role, theme::ink_secondary(), false, face);
    // The search field (a real filter over the tab names); its placeholder in the secondary ink.
    let foc = st.search;
    let c = cs(st, SEARCH_X, SEARCH_Y, SEARCH_W, SEARCH_H, foc);
    let q = st.query.clone();
    draw::text_field(&mut sf(st), e(SEARCH_X), e(SEARCH_Y), ew(SEARCH_X, SEARCH_W), q.as_bytes(), c);
    if q.is_empty() && !foc {
        text_at(st, SEARCH_X + draw::DIMS.field_pad_x, SEARCH_Y + (SEARCH_H - ch) / 2, SEARCH_X + SEARCH_W, "Search", theme::ink_secondary(), false, face);
    }
    // The tab rows: the selected one a full-width accent pill with the light ink, the hovered one washed.
    // MOTIONKIT (B648): the pill is ONE shape that glides between rows — the washes first, then the pill at its
    // glided y, then every icon and label; a label takes the light ink while the pill covers its row's centre.
    let (list, n) = col_list(&st.query);
    st.col_top = col_clamp(st.col_top, n);
    let top = st.col_top;
    let vis = top..(top + COL_VIS).min(n);
    let pill = vis.clone().find(|&i| list[i] as usize == tab).map(|i| {
        let (now, ph) = (st.now, dp(PILL_H));
        let py = st.glides.to(mkey(MK_PILL, 0, 0, 0), e(pill_y(i, top)), now, draw::motion::dur(draw::motion::BASE), draw::motion::Curve::Standard);
        Rect::new(e(PILL_X), py, ew(PILL_X, cw), ph)
    });
    for i in vis.clone() {
        let k = list[i] as usize;
        let state = draw::ControlState { hover: st.col_hover == Some(k), pressed: false, disabled: false, focused: false };
        draw::sidebar_row(&mut sf(st), e(PILL_X), e(pill_y(i, top)), ew(PILL_X, cw), b"", false, state);
    }
    if let Some(p) = pill {
        let state = draw::ControlState { hover: st.col_hover == Some(tab), pressed: false, disabled: false, focused: st.strip && st.kbd };
        draw::sidebar_row(&mut sf(st), p.x, p.y, p.w, b"", true, state);
    }
    for i in vis {
        let k = list[i] as usize;
        let y = pill_y(i, top);
        let on = pill.is_some_and(|p| { let cy = e(y) + ew(y, PILL_H) / 2; cy >= p.y && cy < p.bottom() });
        {
            let mut s = sf(st);
            let iy = y + (PILL_H - ICON) / 2;
            tab_icon(&mut s, k, e(ICON_X), e(iy), ew(ICON_X, ICON), None);
        }
        let ink = if on { theme::ink_on_accent() } else { theme::text_primary() };
        text_at(st, COL_LABEL_X, y + (PILL_H - ch) / 2, COL_W - PILL_X, TAB_NAMES[k], ink, k == tab, face);
    }
    if n == 0 {
        text_at(st, ICON_X, pill_y(0, 0) + (PILL_H - ch) / 2, COL_W - PILL_X, "No results", theme::ink_secondary(), false, face);
    }
    if n > COL_VIS {
        // The scroll thumb: where the visible rows sit in the whole column.
        let track = h - COL_PAD - 8;
        let (ty, th) = (COL_PAD + top * track / n, (COL_VIS * track / n).max(8));
        let r = Rect::new(e(COL_W - 6), e(ty), ew(COL_W - 6, 3), ew(ty, th));
        draw::fill_rrect(&mut sf(st), r, r.w / 2, Ink::argb(theme::control_edge()));
    }
}

fn paint(st: &mut State, v: &Values) {
    let w = st.w;
    st.hits.clear();
    st.now = crate::arch::ms(); st.tab = (v.tab as usize).min(TABS - 1); // MOTIONKIT (B648): this frame's clock and tab, read once
    {
        let mut s = sf(st);
        let all = Rect::new(0, 0, s.w as i32, s.h as i32);
        draw::fill(&mut s, all, theme::surface_window());
    }
    let tab = (v.tab as usize).min(TABS - 1);
    paint_column(st, tab);
    // The pane's title band: the tab's name in the title face, bold (DESIGN-LANGUAGE §3.4: the pane title).
    let tf = super::text::Face::Title; // FONTSCRAFT (B640): the pane title in the title face (owed from the fold, A11YPANE)
    let tc = super::metrics::lcell_h(tf);
    text_at(st, BOX_X, (PANE_Y - tc) / 2, w, TAB_NAMES[tab], theme::text_primary(), true, tf);
    match v.tab {
        0 => paint_general(st, v),
        1 => paint_users(st),
        2 => paint_display(st, v),
        4 => paint_login(st), 5 => paint_appearance(st), // APPEARANCE (B408)
        6 => paint_trackpad(st), // TRACKPADPANE (B412): TP_TAB = 6 (v.tab is u8)
        t if t as usize == FT_TAB => paint_filetypes(st), // FILETYPES (B423)
        t if t as usize == NP_TAB => paint_notifypane(st), // NOTIFYPANE (B435)
        t if t as usize == A11Y_TAB => paint_a11y(st), // A11YPANE (B655)
        t if t as usize == PRIV_TAB => paint_privacy(st), // REASK (B681)
        t if t as usize == DT_TAB => paint_datetime(st), // TZLOCALE (B701)
        t if t as usize == KB_TAB => paint_keyboard(st), // KEYMAPS (B704)
        t if t as usize == STORAGE_TAB => { paint_storage(st); #[cfg(all(target_arch = "x86_64", feature = "wc", feature = "unafs"))] backup::paint(st); } // SMARTLINE (B708)
        t if t as usize == SEC_TAB => paint_security(st), t if t as usize == HW_TAB => paint_hardware(st), // AUTOLOCK (B712) · HWTAB (B737): the census list, its own tab. LINE-NEUTRAL fold
        _ => paint_about(st),
    }
    if let Some((t, r)) = st.found { if t as usize == tab { found_ring(st, r as usize); } } // SETTINGSMAP (B673): the row the search opened
    // SETTINGSFILES (B407, R98): the pane's file, as a link (the accent ink) that reveals `<home>/settings` in Quarry.
    if let Some((r, x, t)) = super::settingsfiles::footer(v.tab as usize, ROWS, CTRL_R, LABEL_X) {
        txt_ink(st, x, r, &t, theme::accent());
    }
    st.hot = hit_at(&st.hits, st.ptr);
}

fn paint_general(st: &mut State, v: &Values) {
    // BRIGHTFLOOR M5: Brightness moved to the Display tab. SETTINGSCRAFT: Sound · Desktop · Account · Dock, four boxes.
    groups(st, &[(0, 1), (2, 4), (5, 5), (6, 8)]); // SETTINGSMAP (B673): the Dock box holds the pins row (8)
    let (k, sel) = (st.kbd && !st.strip, st.sel);
    let fo = move |i: usize| k && sel == i;
    txt(st, LABEL_X, 0, "Volume");
    slider(st, 0, v.vol as usize, 16, fo(1));
    txt2(st, VAL_X, 0, &alloc::format!("{}/16", v.vol));
    txt(st, LABEL_X, 1, "Mute");
    tog(st, 1, TOG_X, v.mute, fo(2)); #[cfg(all(any(feature = "uvc", feature = "hda-tone"), not(all(target_arch = "x86_64", feature = "btc"))))] { let (a, b, k) = crate::audio::picker(); seg(st, 1, &[a.as_bytes(), b.as_bytes()], k, false); } #[cfg(all(any(feature = "uvc", feature = "hda-tone"), target_arch = "x86_64", feature = "btc"))] { let (l, k) = crate::audio::outputs(); let segs: Vec<&[u8]> = l.iter().map(|x| x.as_bytes()).collect(); seg(st, 1, &segs, k, false); } // A2DP (B739): HDA | USB | BT <device> while a headset is on a link (LINE-NEUTRAL append, code first). UAC1 (rmbp-ledger B706): the Sound box's output picker, HDA internal | USB <vid:pid>, in the Mute row's free track (LINE-NEUTRAL append, code first)
    txt(st, LABEL_X, 2, "Pointer");
    btn_f(st, 2, TRACK_X, "Trackpad", false, fo(4)); // TRACKPADPANE (B412): the R75 3-step became the Trackpad tab's speed slider
    txt2(st, TRACK_X + BTN_W + 12, 2, &alloc::format!("speed {}/10", TPV.lock().speed));
    txt(st, LABEL_X, 3, "Wallpaper");
    let (focus, wall) = (!st.strip && st.sel == 5, v.wall.clone());
    field(st, 3, &wall, focus);
    btn_f(st, 4, TRACK_X, "Apply", false, fo(6));
    btn_f(st, 4, TRACK_X + BTN_W + 10, "Off", false, fo(7));
    txt(st, LABEL_X, 5, "Account");
    let who = user_name().unwrap_or_else(|| String::from("(no session)"));
    txt2(st, TRACK_X, 5, &who);
    btn_f(st, 5, VAL_X - 30, "Password", false, fo(8));
    paint_dock_rows(st); // NOTIFYPANE (B435): Do Not Disturb moved to the Notifications tab. DOCK2 (B394): rows 6/7 — the dock's edge and auto-hide
}

fn paint_display(st: &mut State, v: &Values) {
    groups(st, &[(0, 1), (2, 2), (3, 3), (4, 7), (8, 8)]);
    let (k, sel) = (st.kbd && !st.strip, st.sel);
    let fo = move |i: usize| k && sel == i;
    // BRIGHTFLOOR M5: Brightness lives here (row 0) — where Peter looked for it on flight 19.
    txt(st, LABEL_X, 0, "Brightness");
    // BRIGHTSLIDER M2 (B377): the knob and the % are the REGISTER's (its last readback), never the window's copy.
    let (braw, bmax) = (crate::video::backlight::cur_raw(), crate::video::backlight::panel_range());
    PAINTED_RAW.store(braw, Ordering::Relaxed);
    slider(st, 0, braw.min(bmax) as usize, bmax as usize, fo(0));
    txt2(st, VAL_X, 0, &alloc::format!("{}%", crate::video::backlight::pct_of(braw, bmax)));
    txt(st, LABEL_X, 1, "Blank screen");
    slider(st, 1, idle_index(v.idle_min), IDLE_STEPS.len() - 1, fo(3));
    let it = if v.idle_min == 0 { String::from("never") } else { alloc::format!("{} min", v.idle_min) };
    txt2(st, VAL_X, 1, &it);
    txt(st, LABEL_X, 2, "Resolution"); // PREFSUI (R93): the popup of the modes the display path can set (was the read-only UI scale row)
    mode_field(st);
    // SETTINGSMAP (B673): `system.power.lowbat_shutdown_pct` gets its row here, in place of the fixed-clock caption (a row
    // that held no key): - / + in steps of LOWBAT_STEP, 0 = off; the store write is drained by `lowbat_service`.
    txt(st, LABEL_X, LOWBAT_ROW, "Low battery shutdown");
    btn(st, LOWBAT_ROW, TRACK_X, "-");
    btn(st, LOWBAT_ROW, TRACK_X + BTN_W + 10, "+");
    let lb = lowbat_now();
    txt2(st, TRACK_X + 2 * BTN_W + 24, LOWBAT_ROW, &if lb == 0 { String::from("Off") } else { alloc::format!("at {}%", lb) });
    // KERNELFONT M3 (B359) -> KERNELFONT2 M4 (B363): the Font row is a PICKER — the family as three segments and the
    // size as - / + — writing Principia's `system.display.font` / `font_size` (`video::text` follows them within a
    // second and every window repaints once); the faces drawing now and a sample line IN the UI face below.
    txt(st, LABEL_X, 4, "Font");
    let (fam, size) = (font_fam(), font_size());
    seg(st, 4, &[FONT_FAMS[0].as_bytes(), FONT_FAMS[1].as_bytes(), FONT_FAMS[2].as_bytes()], fam, fo(9));
    txt2(st, VAL_X, 4, &super::text::face_name(super::text::Face::Ui));
    txt(st, LABEL_X, 5, "Font size");
    btn_f(st, 5, TRACK_X, "-", false, fo(10));
    btn_f(st, 5, TRACK_X + BTN_W + 10, "+", false, fo(10));
    txt2(st, TRACK_X + 2 * BTN_W + 24, 5, &alloc::format!("{} px", size));
    txt(st, LABEL_X, 6, "Console");
    let (cw, chh) = super::text::grid_cell();
    let s2 = super::dpi::scale_x2();
    txt2(st, TRACK_X, 6, &alloc::format!("{} in {}x{} cells, {} ppi x{}", super::text::face_name(super::text::Face::Grid), cw, chh, super::dpi::ppi(), super::dpi::scale_str(s2)));
    txt(st, LABEL_X, 7, "Sample");
    txt(st, TRACK_X, 7, SAMPLE);
    if st.modes_open { mode_list(st); } // PREFSUI: the open popup paints over the rows below it
}

/// KERNELFONT M3: the Display tab's sample line.
pub const SAMPLE: &str = "The quick brown fox jumps over the lazy dog 0123456789";

/// About: one centred card — the mark, the name, the version, then the facts as `label: value` around the centre.
fn paint_about(st: &mut State) { #[cfg(all(target_arch = "x86_64", feature = "installdemo"))] sysimg::paint(st); // UPDATEUI (B726): the System image rows under the card (drawn first: the card's box ends above them). LINE-NEUTRAL fold
    let ver = option_env!("UNAOS_GIT_SHA").unwrap_or("dev build");
    let (y0, y1) = (row_top(0) + GAP, row_top(9) - GAP); #[cfg(all(target_arch = "x86_64", feature = "installdemo"))] let y1 = y1.min(row_top(sysimg::FIRST) - GAP); // UPDATEUI (B726): the card ends above the System image rows. LINE-NEUTRAL fold
    let mark = 64; #[cfg(all(target_arch = "x86_64", feature = "installdemo"))] let mark = sysimg::card_mark(mark); // UPDATEUI: the compact card drops the mark. LINE-NEUTRAL fold
    let cx = PANE_X + PANE_W / 2;
    {
        let mut s = sf(st);
        draw::group_box(&mut s, Rect::new(e(BOX_X), e(y0), ew(BOX_X, BOX_W), e(y1) - e(y0)), b"");
        #[cfg(not(all(target_arch = "x86_64", feature = "installdemo")))] tab_icon(&mut s, 3, e(cx - mark / 2), e(y0 + 24), ew(cx - mark / 2, mark), Some(theme::accent())); // UPDATEUI (B726): the compact card (installdemo) draws no mark. LINE-NEUTRAL fold
    }
    let (tf, uf) = (super::text::Face::Title, super::text::Face::Ui); // FONTSCRAFT (B640): the About card's name in the title face
    let (tc, uc) = (super::metrics::lcell_h(tf), super::metrics::lcell_h(uf));
    let nx = LABEL_X + mark + 12;
    text_at(st, nx, y0 + 16, cx - 8, "UnaOS", theme::text_primary(), true, tf);
    text_at(st, nx, y0 + 16 + tc + 2, cx - 8, ver, theme::ink_secondary(), false, uf);
    let mut y = y0 + 16 + mark + 14;
    text_at(st, LABEL_X, y, cx - 8, "Safe mode: hold Shift after login", theme::ink_secondary(), false, uf);
    y += uc + 4;
    text_at(st, LABEL_X, y, cx - 8, "(display prefs reset; or an UNAOS_PREFS_RESET=1 image)", theme::ink_secondary(), false, uf);
    #[cfg(target_arch = "x86_64")]
    let cpus = alloc::format!("{}", crate::arch::x86_64::acpi::cpu_count());
    #[cfg(not(target_arch = "x86_64"))]
    let cpus = String::from("n/a");
    let (a, b) = crate::allocator::heap_bounds();
    let mem = alloc::format!("{} MiB kernel heap", b.saturating_sub(a) >> 20);
    let up = crate::clock::uptime_secs().map(|u| alloc::format!("{}h {}m {}s", u / 3600, (u / 60) % 60, u % 60)).unwrap_or_else(|| String::from("n/a"));
    let board = if cfg!(target_arch = "x86_64") { "x86_64 (UEFI)" } else { "aarch64" };
    let facts: [(&str, &str); 4] = [
        ("Board", board), ("CPUs", &cpus),
        ("Memory", &mem), ("Uptime", &up),
    ];
    let mut y = y0 + 16;
    for (k, val) in facts {
        let kw = super::metrics::ladvance(k.as_bytes(), false, uf);
        text_at(st, (cx - 8).saturating_sub(kw), y, cx, k, theme::ink_secondary(), false, uf);
        text_at(st, cx + 8, y, CTRL_R, val, theme::text_primary(), false, uf);
        y += uc + 10; #[cfg(all(target_arch = "x86_64", feature = "installdemo"))] { y -= sysimg::FACT_TIGHTEN; } // UPDATEUI (B726): the compact card's facts. LINE-NEUTRAL fold
    }
    // HWTAB (B737): the Hardware list left About for its own tab (`HW_TAB`, file tail); About keeps the card and the System image group
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
    /// FIRSTUSER (R100): the new user's role — the form's Admin toggle (standard by default).
    admin: bool,
}

impl UsersUi {
    fn new() -> Self {
        UsersUi { sel: 0, form: false, focus: 0, name: String::new(), pw: String::new(), pw2: String::new(), confirm: None, msg: String::new(), admin: false }
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
    { crate::fs::users::privileged() } // FIRSTUSER (R100): the administrator's session (or the system principal before login)
    #[cfg(not(feature = "login"))]
    { false }
}

fn users_say(op: &str, name: &str, ok: bool, reason: &str) {
    if !ok { U_REFUSED.fetch_add(1, Ordering::Relaxed); }
    serial_println!("[settings] users op={} name={} ok={} reason={}", op, name, ok as u8, reason);
}

/// Add a user (root only): the create-user form's rules, then the `adduser` path and a first password.
fn users_add(name: &str, pw: &str, pw2: &str, admin: bool) -> Result<(), &'static str> {
    #[cfg(feature = "login")]
    {
        use crate::fs::users;
        users::admin_authority("settings-users-add")?; // FIRSTUSER (R100): `[auth] admin=<name> for=settings-users-add`
        users::create_user_rules(name.as_bytes(), pw.as_bytes())?;
        if pw != pw2 { return Err("Passwords do not match"); }
        users::adduser_commit(name.as_bytes())?;
        if users::set_first_password(name.as_bytes(), pw.as_bytes()).is_err() { return Err("Could not save the password"); }
        if admin && users::set_role(name.as_bytes(), users::Role::Admin).is_err() { return Err("Created, but not as an administrator"); }
        U_ADDED.fetch_add(1, Ordering::Relaxed);
        return Ok(());
    }
    #[cfg(not(feature = "login"))]
    { let _ = (name, pw, pw2, admin); Err("login-feature-off") }
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
        let reset = crate::video::crystal::login::open_change_password(name.as_bytes()); // SETTINGSREKEY (B483): own row = change (old password, ring re-keyed); another's = reset
        if reset { if let Some(s) = STATE.lock().as_mut() { s.u.msg = alloc::format!("{}: {}", name, crate::video::crystal::login::RESET_LINE); } }
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
        txt2(st, LABEL_X, 0, "New user");
        groups(st, &[(1, 3)]);
        txt(st, LABEL_X, 1, "Name");
        let (n, f) = (st.u.name.clone(), st.u.focus == 0);
        field(st, 1, &n, f);
        txt(st, LABEL_X, 2, "Password");
        let (p, f) = ("*".repeat(st.u.pw.len()), st.u.focus == 1);
        field(st, 2, &p, f);
        txt(st, LABEL_X, 3, "Retype");
        let (p, f) = ("*".repeat(st.u.pw2.len()), st.u.focus == 2);
        field(st, 3, &p, f);
        btn_f(st, 4, TRACK_X, "Create", true, false);
        btn(st, 4, TRACK_X + BTN_W + 10, "Cancel");
        // FIRSTUSER (R100): admin or standard — Settings > Users is the one place. SETTINGSCRAFT: a check box and its
        // label inside the old button's press span.
        let ax = TRACK_X + 2 * (BTN_W + 10);
        let adm = st.u.admin;
        check(st, 4, ax, adm);
        txt(st, ax + draw::DIMS.check + 8, 4, "Admin");
    } else {
        txt2(st, LABEL_X, 0, &alloc::format!("Users ({})", list.len()));
        if root { btn(st, 0, TRACK_X, "Add user"); } else { txt2(st, TRACK_X, 0, "Only an administrator can change the list."); }
        let shown = list.len().min(8);
        if shown > 0 { groups(st, &[(1, shown)]); }
        let sel = st.u.sel.min(list.len().saturating_sub(1));
        let face = super::text::Face::Ui;
        for (i, (nm, unset)) in list.iter().enumerate().take(8) {
            let r = 1 + i;
            let tag = if *nm == me { " (you)" } else if *unset { " (no password)" } else if users_is_admin(nm) { " (admin)" } else { "" };
            if i == sel && !st.strip { row_wash(st, r); if st.kbd { row_ring(st, r); } } // A11YTREE (B675): a Tab stop on the list shows the kit's ring (DESIGN-LANGUAGE §2.7)
            let d = 24;
            avatar(st, LABEL_X, mid_y(r, d), d, nm);
            let ch = super::metrics::lcell_h(face);
            let pen = text_at(st, LABEL_X + d + 10, row_top(r) + (ROW_H - ch) / 2, TRACK_X - 8, nm, theme::text_primary(), false, face);
            if !tag.is_empty() { text_at(st, pen, row_top(r) + (ROW_H - ch) / 2, TRACK_X - 8, tag, theme::ink_secondary(), false, face); }
            if st.u.confirm.as_deref() == Some(nm.as_str()) {
                txt2(st, TRACK_X + 130, r, "Delete?");
                btn_f(st, r, TRACK_X + 200, "Yes", true, false);
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
    txt2(st, LABEL_X, 9, &m);
}

fn users_form_submit() {
    let (n, p, p2, adm) = { let s = STATE.lock(); match s.as_ref() { Some(s) => (s.u.name.clone(), s.u.pw.clone(), s.u.pw2.clone(), s.u.admin), None => return } };
    let r = users_add(&n, &p, &p2, adm);
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
    surf.resize(len, theme::surface_content());
    if is_open() { close(); }
    // Pick up the file's values (and apply them) if the login hook has not yet.
    if user_name().is_some() && LOADED_FOR.lock().is_empty() { service(); }
    let (_s, ow, oh) = wm::spawn_geometry_native(sw, sh).ok_or_else(|| String::from("geometry unavailable"))?;
    let wtop = crate::ui_status::top_chrome_h(pw, ph);
    let ox = pw.saturating_sub(ow) / 2;
    let oy = wtop + ph.saturating_sub(wtop).saturating_sub(crate::ui_status::chrome_h(ph)).saturating_sub(oh) / 2;
    let tab0 = CUR.lock().tab as usize;
    let (sel0, strip0) = take_at(tab0); // LAUNCHER (B417): a launcher pick opens on its row
    let mut st = State::new(w, h, surf, sel0, strip0, col_scrolled(0, tab0, TABS));
    LAST_PTR.store(u64::MAX, Ordering::Relaxed); // SETTINGSCRAFT: the hover pass reads the pointer afresh over the new window
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
    serial_println!("[settings] tabs=left col_w={} rows={} sel={}", COL_W, TABS, TAB_NAMES[tab0.min(TABS - 1)]); // SETTINGSTABS (B623)
    witness(true);
    Ok(())
}

/// Close the window; the surface is freed after the row stops naming it.
pub fn close() {
    let id = WIN.swap(wm::WIN_NONE, Ordering::Relaxed);
    if id == wm::WIN_NONE { return; }
    if DRAG.lock().take().is_some() | TPD.lock().take().is_some() { super::capture::cancel(); } // PREFSUI: a drag dies with its window (TRACKPADPANE: the speed slider's too)
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
        st.modes_open = false;
        st.found = None;
        if let Some(&c) = tab_ctrls(t).first() { st.sel = c; }
        let (list, n) = col_list(&st.query); // SETTINGSCRAFT: the chosen row is kept in view of the (filtered) column
        if let Some(p) = list[..n].iter().position(|&k| k as usize == t) { st.col_top = col_scrolled(st.col_top, p, n); }
    }
    serial_println!("[settings] tab={}", TAB_NAMES[t]);
    if t == FT_TAB { crate::fs::assoc::owe_view(); } // FILETYPES (B423): the pane's rows are read on the service pass
    persist(6);
    repaint();
}

pub(super) fn cur_tab() -> usize { CUR.lock().tab as usize } // A11YTREE (B675): `pub(super)` with TAB_NAMES, so the accessibility tree names the tab rows from this table

// ── SETTINGSTABS (rmbp-ledger B623, R118) — the tab column ────────────────────────────────────────────────────────
// "settings tabs typically go on the left since there get to be a lot of them." Rows are `COL_ROW` tall from `COL_PAD`;
// `COL_VIS` show at once and the column scrolls. SETTINGSCRAFT (B630): the column lists the tabs whose name holds the
// search text ([`col_list`]); positions below are positions in THAT list. Pure helpers, shared by the paint, the press
// router, the wheel and the fixture.

/// Does tab name `name` hold `q` (ASCII case-insensitive; the empty query holds everywhere)?
fn name_matches(name: &str, q: &str) -> bool {
    q.is_empty() || name.as_bytes().windows(q.len()).any(|w| w.eq_ignore_ascii_case(q.as_bytes()))
}

/// The tabs the column lists for the search text `q`, in column order, and how many. SETTINGSMAP (B673): a tab is
/// listed when its name holds `q` or one of its rows' labels does ([`MAP`]).
fn col_list(q: &str) -> ([u8; TABS], usize) {
    let mut out = [0u8; TABS];
    let mut n = 0;
    for (k, name) in TAB_NAMES.iter().enumerate() {
        if name_matches(name, q) || MAP.iter().any(|r| r.tab == k && hold_rank(r.label, q).is_some()) { out[n] = k as u8; n += 1; }
    }
    (out, n)
}

/// The first visible row held to a list of `n`.
fn col_clamp(top: usize, n: usize) -> usize {
    top.min(n.saturating_sub(COL_VIS))
}

/// The column's first visible row so that list position `pos` is in view, starting from `top`.
fn col_scrolled(top: usize, pos: usize, n: usize) -> usize {
    let top = if pos < top { pos } else if pos >= top + COL_VIS { pos + 1 - COL_VIS } else { top };
    col_clamp(top, n)
}

/// The tab under logical `cy` in a column listing `list[..n]` from row `top` (`None` = the inset or past the last).
fn col_tab_at(cy: usize, top: usize, list: &[u8; TABS], n: usize) -> Option<usize> {
    if cy < COL_PAD { return None; }
    let r = (cy - COL_PAD) / COL_ROW;
    let i = top + r;
    (r < COL_VIS && i < n).then(|| list[i] as usize)
}

/// The last pointer position the hover pass read (`x << 32 | y`; `u64::MAX` = read it afresh).
static LAST_PTR: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(u64::MAX);

/// Pointer motion over the window: light the column row and the control under it, and repaint only when either
/// changed. Never consumes. SETTINGSCRAFT: called by [`hover_poll`] on the desktop's service pass (the motion itself
/// is routed in `arch/x86_64/syscall.rs`, which this arc does not touch; the dock's hover reads the cursor the same way).
pub fn hover_route(x: i32, y: i32) -> bool {
    let id = WIN.load(Ordering::Relaxed);
    if id == wm::WIN_NONE { return false; }
    let Some(info) = wm::info(id) else { return false };
    let inside = x >= info.x as i32 && y >= info.y as i32 && matches!(wm::hit_test(x, y), Some((w, _, _)) if w == id);
    let p = if inside {
        let sc = info.scale.max(1);
        Some((super::metrics::to_logical((x as usize - info.x) / sc), super::metrics::to_logical((y as usize - info.y) / sc)))
    } else { None };
    let changed = {
        let mut g = STATE.lock();
        let Some(s) = g.as_mut() else { return false };
        let (list, n) = col_list(&s.query);
        let col = p.and_then(|(cx, cy)| if cx < COL_W { col_tab_at(cy, s.col_top, &list, n) } else { None });
        let hot = hit_at(&s.hits, p);
        s.ptr = p;
        let c = s.col_hover != col || s.hot != hot;
        s.col_hover = col;
        s.hot = hot;
        c
    };
    if changed { repaint(); }
    false
}

/// SETTINGSCRAFT: the hover and pressed-face pass, chained beside [`service`] from `quarry::live::service` (one line
/// there). Ends a press's pressed face after [`PRESS_MS`]; reads the cursor and, when it moved, asks [`hover_route`].
/// A closed window costs one load.
pub fn hover_poll() {
    if !is_open() { return; }
    let now = crate::arch::ms();
    let done = STATE.lock().as_mut().is_some_and(|s| {
        let pressed = match s.press {
            Some((_, _, t)) if now.saturating_sub(t) >= PRESS_MS => { s.press = None; true }
            _ => false,
        };
        let running = s.glides.running(); // MOTIONKIT (B648): a frame is owed while a value glides — one per 16 ms, none idle
        s.frame.due(now, running) | pressed
    });
    if done { repaint(); }
    let Some(i) = crate::video::panel_info_nonblocking() else { return };
    let (x, y) = crate::pal::cursor::pos(i.width as i32, i.height as i32);
    let k = ((x as u32 as u64) << 32) | y as u32 as u64;
    if LAST_PTR.swap(k, Ordering::Relaxed) == k { return; }
    hover_route(x, y);
}

/// SETTINGSCRAFT: a wheel detent over the window (`pal::Event::Wheel`, reaching [`key_route`] first in
/// `quarry::live::key_route`'s chain — scroll-under-pointer, never focus-gated, as QSCROLL rules). Over the column it
/// scrolls the rows (positive = away = toward the top, QSCROLL's sign); over the File Types pane it pages. `true` when
/// the pointer is over this window (consumed), `false` otherwise (the event falls through untouched).
fn wheel(d: i8) -> bool {
    let id = WIN.load(Ordering::Relaxed);
    if id == wm::WIN_NONE || d == 0 { return false; }
    let Some(i) = crate::video::panel_info_nonblocking() else { return false };
    let (x, y) = crate::pal::cursor::pos(i.width as i32, i.height as i32);
    match wm::hit_test(x, y) {
        Some((w, _, _)) if w == id => {}
        _ => return false,
    }
    let Some(info) = wm::info(id) else { return true };
    if x < info.x as i32 || y < info.y as i32 { return true; }
    let cx = super::metrics::to_logical((x as usize - info.x) / info.scale.max(1));
    let step = d.unsigned_abs() as usize;
    let (area, moved) = if cx < COL_W {
        let mut g = STATE.lock();
        let Some(s) = g.as_mut() else { return true };
        let n = col_list(&s.query).1;
        let top = s.col_top;
        let next = col_clamp(if d > 0 { top.saturating_sub(step) } else { top + step }, n);
        s.col_top = next;
        ("column", next != top)
    } else if cur_tab() == FT_TAB {
        let pages = crate::fs::assoc::view().len().div_ceil(FT_ROWS).max(1);
        let page = FT_PAGE.load(Ordering::Relaxed).min(pages - 1);
        let next = if d > 0 { page.saturating_sub(step) } else { (page + step).min(pages - 1) };
        FT_PAGE.store(next, Ordering::Relaxed);
        ("pane", next != page)
    } else {
        if cur_tab() == HW_TAB { hw_wheel(d, step) } else { ("pane", false) } // HWREPORT (B725) -> HWTAB (B737): the Hardware list scrolls under the wheel, on its own tab
    };
    serial_println!("[settings] wheel area={} d={} moved={}", area, d, moved as u8);
    if moved { repaint(); }
    true
}

/// Keys and actions, only while this window holds focus. `true` when consumed.
pub fn key_route(ev: crate::pal::Event) -> bool {
    use crate::video::keymap::Action;
    if let crate::pal::Event::Wheel(d) = ev { return wheel(d); } // SETTINGSCRAFT: the wheel goes by the pointer, not the focus
    if !is_open() || wm::focus_asid() != OWNER { return false; }
    let searching = STATE.lock().as_mut().map(|x| { x.kbd = true; x.found = None; x.search }).unwrap_or(false); // SETTINGSCRAFT: a key shows the focus ring
    if searching { return search_key(ev); }
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
                    // Up: previous control; from the first one, the tab column. SETTINGSTABS: on the column, the previous tab.
                    if strip {
                        strip_step(tab, false);
                    } else if tab == 1 {
                        let mut g = STATE.lock();
                        if let Some(x) = g.as_mut() { if x.strip { } else if x.u.sel > 0 { x.u.sel -= 1; } else { x.strip = true; } }
                        drop(g);
                        repaint();
                    } else {
                        let p = ctrls.iter().position(|&k| k == s).unwrap_or(0);
                        if p == 0 { if let Some(x) = STATE.lock().as_mut() { x.strip = true; } repaint(); } else { select(ctrls[p - 1]); }
                    }
                    true
                }
                0x1F if strip => { strip_step(tab, true); true } // SETTINGSTABS: Down on the column, the next tab (SETTINGSCRAFT: of those the search lists)
                0x09 if !strip => { if let Some(x) = STATE.lock().as_mut() { x.strip = true; } repaint(); true } // SETTINGSTABS: Tab, pane -> column
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

/// SETTINGSCRAFT: Up / Down on the column — the previous / next tab the (filtered) column lists; Up from the first
/// row takes the search field.
fn strip_step(tab: usize, down: bool) {
    let q = STATE.lock().as_ref().map(|x| x.query.clone()).unwrap_or_default();
    let (list, n) = col_list(&q);
    let pos = list[..n].iter().position(|&k| k as usize == tab);
    let next = match (pos, down) {
        (None, _) => list[..n].first().copied(),
        (Some(p), true) => list[..n].get(p + 1).copied(),
        (Some(0), false) => { if let Some(x) = STATE.lock().as_mut() { x.search = true; } repaint(); return; }
        (Some(p), false) => Some(list[p - 1]),
    };
    if let Some(k) = next { switch_tab(k as usize); }
}

/// SETTINGSCRAFT: a key while the search field holds the keys — type to filter, Backspace, Enter opens the first
/// match, Esc clears and leaves, Down leaves to the column. Every key is consumed (the field has the keyboard).
/// SETTINGSMAP (B673): Return opens [`search_best`]'s hit (a row, or a tab); the first listed tab only when none.
fn search_key(ev: crate::pal::Event) -> bool {
    let (c, ch) = match ev { crate::pal::Event::Key(c) => (c, '\0'), crate::pal::Event::Char(ch) => (0, ch), _ => return true }; // KEYCHARS (B718): a layout character (`ü`, `é`) is typed into the query by the arm below
    let mut pick = None;
    let mut hit = None;
    {
        let mut g = STATE.lock();
        let Some(x) = g.as_mut() else { return true };
        match c {
            0x08 | 0x7F => { x.query.pop(); }
            0x1B => { x.query.clear(); x.search = false; }
            0x0A | 0x0D | 0x1F => {
                let (list, n) = col_list(&x.query);
                x.search = false;
                x.strip = true;
                hit = if c == 0x1F { None } else { search_best(&x.query) };
                if hit.is_none() && n > 0 { pick = Some(list[0] as usize); }
                x.col_top = 0;
            }
            0x20..=0x7e => { if x.query.len() < 24 { x.query.push(c as char); x.col_top = 0; } } _ if ch != '\0' => { if x.query.len() < 24 { x.query.push(ch); x.col_top = 0; } } // KEYCHARS (B718): the Char arm, same 24-byte bound (LINE-NEUTRAL)
            _ => {}
        }
    }
    let q = STATE.lock().as_ref().map(|x| x.query.clone()).unwrap_or_default();
    serial_println!("[settings] search q={} matches={}", q, col_list(&q).1);
    if let Some(h) = hit { open_hit(h, q.len()); return true; }
    match pick {
        Some(t) if t != cur_tab() => switch_tab(t),
        _ => repaint(),
    }
    true
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
    // SETTINGSCRAFT: the press shows its control's pressed face for PRESS_MS (the action's own repaint draws it); a
    // press anywhere but the search field takes the keys back from it; the focus ring hides until the next key.
    let in_search = cx >= SEARCH_X && cx < SEARCH_X + SEARCH_W && cy >= SEARCH_Y && cy < SEARCH_Y + SEARCH_H;
    let (top, query) = match STATE.lock().as_mut() {
        Some(s) => { s.press = Some((cx, cy, crate::arch::ms().max(1))); s.kbd = false; s.search = in_search; s.found = None; (s.col_top, s.query.clone()) }
        None => return true,
    };
    if cx < COL_W { // SETTINGSTABS (B623): a press in the column selects by row
        if in_search { repaint(); return true; }
        if cy < CARD_Y + CARD_H { if cur_tab() != 1 { switch_tab(1); } else { repaint(); } return true; } // the user card opens Users
        let (list, n) = col_list(&query);
        let Some(t) = col_tab_at(cy, top, &list, n) else { repaint(); return true };
        if t != cur_tab() { switch_tab(t); } else { if let Some(st) = STATE.lock().as_mut() { st.strip = true; } repaint(); }
        return true;
    }
    if cy < TOP { repaint(); return true; }
    let row = (cy - TOP) / ROW_H;
    if row >= ROWS { repaint(); return true; }
    if !STATE.lock().as_ref().is_some_and(|s| s.modes_open) && super::settingsfiles::press(cur_tab(), row, cx, ROWS, CTRL_R, LABEL_X) { return true; } // SETTINGSFILES (B407): the pane's `Stored in settings/<domain>` link
    match cur_tab() {
        0 => press_general(row, cx),
        1 => press_users(row, cx), #[cfg(all(target_arch = "x86_64", feature = "installdemo"))] 3 => sysimg::press(row, cx), // UPDATEUI (B726): About > System image. LINE-NEUTRAL fold
        4 => press_login(row, cx), 5 => press_appearance(row, cx), // APPEARANCE (B408)
        TP_TAB => press_trackpad(row, cx), // TRACKPADPANE (B412)
        t if t == FT_TAB => press_filetypes(row, cx), // FILETYPES (B423)
        t if t == NP_TAB => press_notifypane(row, cx), // NOTIFYPANE (B435)
        t if t == A11Y_TAB => press_a11y(row, cx), // A11YPANE (B655)
        t if t == PRIV_TAB => press_privacy(row, cx), // REASK (B681)
        t if t == DT_TAB => press_datetime(row, cx), // TZLOCALE (B701)
        t if t == KB_TAB => press_keyboard(row, cx), // KEYMAPS (B704)
        t if t == SEC_TAB => press_security(row, cx), t if t == HW_TAB => hw_press(row, cy, cx), #[cfg(all(target_arch = "x86_64", feature = "wc", feature = "unafs"))] t if t == STORAGE_TAB => backup::press(row, cx), // AUTOLOCK (B712) · HWTAB (B737): Refresh, or a device line copied to the console. LINE-NEUTRAL fold
        2 => {
            if mode_press(row, cx) { return true; } // PREFSUI (R93): the Resolution dropdown, open or opening
            let on_track = cx >= TRACK_X - 6 && cx <= TRACK_X + TRACK_W + 6;
            if row == 0 && on_track { select(0); bright_click(cx); drag_begin(0, cx); } // PREFSUI (R93): the press sets AND captures — the knob follows the hand until the release
            if row == 1 && on_track { select(3); drag_begin(3, cx); }
            if row == 4 && cx >= TRACK_X && cx < TRACK_X + TRACK_W { select(9); set_font(9, ((cx - TRACK_X) / (TRACK_W / 3)).min(2) as i64); } // KERNELFONT2: the family segments
            if row == 5 && cx >= TRACK_X && cx < TRACK_X + BTN_W { select(10); set_font(10, (font_size() - 1).max(FONT_MIN)); } // KERNELFONT2: size −
            if row == 5 && cx >= TRACK_X + BTN_W + 10 && cx < TRACK_X + 2 * BTN_W + 10 { select(10); set_font(10, (font_size() + 1).min(FONT_MAX)); } // size +
            if row == LOWBAT_ROW && cx >= TRACK_X && cx < TRACK_X + BTN_W { lowbat_step(false); } // SETTINGSMAP (B673): low battery -
            if row == LOWBAT_ROW && cx >= TRACK_X + BTN_W + 10 && cx < TRACK_X + 2 * BTN_W + 10 { lowbat_step(true); } // low battery +
        }
        _ => {}
    }
    true
}

fn press_general(row: usize, cx: usize) {
    // BRIGHTFLOOR M5: rows moved up one (Brightness is on the Display tab).
    match row {
        0 if cx >= TRACK_X - 6 && cx <= TRACK_X + TRACK_W + 6 => { select(1); drag_begin(1, cx); } // PREFSUI (R93): press-and-drag
        1 => { select(2); if cx >= TOG_X && cx < TOG_X + draw::DIMS.toggle_w { let m = CUR.lock().mute; set(2, (!m) as usize); } #[cfg(all(any(feature = "uvc", feature = "hda-tone"), not(all(target_arch = "x86_64", feature = "btc"))))] { if cx >= TRACK_X && cx < TRACK_X + TRACK_W { let _ = crate::audio::sink(if cx < TRACK_X + TRACK_W / 2 { "hda" } else { "usb" }); repaint(); } } #[cfg(all(any(feature = "uvc", feature = "hda-tone"), target_arch = "x86_64", feature = "btc"))] { if cx >= TRACK_X && cx < TRACK_X + TRACK_W { let n = crate::audio::outputs().0.len().max(1); let _ = crate::audio::choose((cx - TRACK_X) * n / TRACK_W); repaint(); } } } // A2DP (B739): the segment under the press, of two or three (LINE-NEUTRAL append, code first). UAC1 (B706): a click on the output picker chooses the sink (Principia `system.audio.output`); LINE-NEUTRAL append, code first. SETTINGSCRAFT: the switch, right-aligned
        2 => { if cx >= TRACK_X && cx < TRACK_X + BTN_W { switch_tab(TP_TAB); } } // TRACKPADPANE (B412): the Trackpad tab holds the speed now
        3 => select(5),
        4 => {
            if cx >= TRACK_X && cx < TRACK_X + BTN_W { select(6); do_wallpaper(false); }
            else if cx >= TRACK_X + BTN_W + 10 && cx < TRACK_X + 2 * BTN_W + 10 { select(7); do_wallpaper(true); }
        }
        5 => { if cx >= VAL_X - 30 && cx < VAL_X - 30 + BTN_W { select(8); change_password(); } }
        6 | 7 => dock_rows_press(row, cx), // DOCK2 (B394)
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
                else if in_b(TRACK_X + 2 * (BTN_W + 10)) { if let Some(s) = STATE.lock().as_mut() { s.u.admin = !s.u.admin; } repaint(); } // FIRSTUSER (R100): the role toggle
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
    if let Some(k) = crate::fs::vfs::home_unwritable(&crate::shell::vfs_mount_table(), &crate::prefs::path()) { serial_println!(":: SETTINGS: controls={} tabs={} store={} -> SKIP reason=home-not-writable({}) ::", CONTROLS, TABS, crate::prefs::path(), k); return; } let before = CUR.lock().clone(); // QEMUSKIP (B613): the save + re-read legs need the store's volume to take a write
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
    // SETTINGSTABS (B623): every tab's column row maps back to that tab, and the pane starts right of the column.
    // SETTINGSCRAFT (B630): ... and the search filters the column by name.
    let (all, n_all) = col_list("");
    let (disp, n_disp) = col_list("DISP");
    let col_ok = PANE_X == COL_W && WIN_W == COL_W + PANE_W && n_all == TABS && n_disp == 1 && disp[0] == 2 && col_list("zzz").1 == 0
        && (0..TABS).all(|k| { let t = col_scrolled(0, k, TABS); col_tab_at(COL_PAD + (k - t) * COL_ROW + COL_ROW / 2, t, &all, TABS) == Some(k) });
    let craft = craft_check();
    let ok = saved && same && live && col_ok && craft.is_ok();
    serial_println!("[settings] fixture opened={} saved={:?} reread_same={} live_idle={} store={} tabs=left col_w={} sel={} col_ok={} craft={} win={}x{}", opened as u8, saved, same as u8, live as u8, crate::prefs::path(), COL_W, TAB_NAMES[(CUR.lock().tab as usize).min(TABS - 1)], col_ok as u8, craft.err().unwrap_or("ok"), WIN_W, WIN_H);
    witness(ok);
}

/// SETTINGSCRAFT (B630) — `craft=`: paint the General pane OFFSCREEN in the Light set and again in the Dark set (the
/// live set, accent and highlight restored after), and read the pixels back:
/// * **icons** — every tab row's tile is a rounded tile (its corner pixel is not the tile's colour) with a glyph on it
///   (inside it, both the tile's colour and other ink);
/// * **hairline** — the first group box's top edge is the hairline over the box's fill, the window's face above it and
///   the box's fill below it; the separator between its rows is the hairline from the row inset and the fill left of it;
/// * **round** — the box's and the selected pill's corner pixels are the ground under them: no bare 1-px-bordered
///   rectangle (the old raw boxes had a square, inked corner);
/// * **sets** — the column differs from the pane in both sets and the pane's face differs between them.
///
/// `Ok` or the first failing check's name (`-light` / `-dark` appended).
#[cfg(feature = "witness")]
fn craft_check() -> Result<(), &'static str> {
    let (dark0, acc0, hl0) = (theme::is_dark(), theme::accent_index(), theme::highlight_index());
    let mut face = [0u32; 2];
    let mut r = Ok(());
    for (i, dark) in [false, true].into_iter().enumerate() {
        theme::set(dark, acc0, hl0);
        match craft_probe() {
            Ok(f) => face[i] = f,
            Err(e) => { r = Err(if dark { craft_dark(e) } else { e }); break; }
        }
    }
    theme::set(dark0, acc0, hl0);
    if r.is_ok() && face[0] == face[1] { r = Err("sets"); }
    r
}

#[cfg(feature = "witness")]
fn craft_dark(e: &'static str) -> &'static str {
    match e {
        "icons" => "icons-dark",
        "hairline" => "hairline-dark",
        "round" => "round-dark",
        "sets" => "sets-dark",
        x => x,
    }
}

/// One set's probe: the General pane painted into a fresh surface; returns the pane's face.
#[cfg(feature = "witness")]
fn craft_probe() -> Result<u32, &'static str> {
    let (pw, ph) = (super::metrics::size(WIN_W), super::metrics::size(WIN_H));
    let mut surf: Vec<u32> = Vec::new();
    if surf.try_reserve_exact(pw * ph).is_err() { return Err("oom"); }
    surf.resize(pw * ph, 0);
    let mut st = State::new(WIN_W, WIN_H, surf, 1, true, 0);
    let mut v = CUR.lock().clone();
    v.tab = 0;
    paint(&mut st, &v);
    let px = |x: i32, y: i32| -> u32 { if x < 0 || y < 0 { return u32::MAX; } st.surf.get(y as usize * pw + x as usize).map_or(u32::MAX, |p| p & 0x00FF_FFFF) };
    let (face, sb, fill) = (theme::surface_window(), sidebar_bg(), theme::group_fill());
    let hair = draw::blend(fill, theme::text_primary(), theme::HAIRLINE_ALPHA);
    if sb == face { return Err("sets"); }
    // icons
    for k in 0..TABS {
        let iy = pill_y(k, 0) + (PILL_H - ICON) / 2;
        let (x0, y0, t) = (e(ICON_X), e(iy), ew(ICON_X, ICON));
        let tile = icon_tile(k);
        if px(x0, y0) == tile { return Err("icons"); }
        let (mut on, mut ink) = (0, 0);
        for yy in y0 + 1..y0 + t - 1 {
            for xx in x0 + 1..x0 + t - 1 {
                if px(xx, yy) == tile { on += 1; } else { ink += 1; }
            }
        }
        if on < 4 || ink < 3 { return Err("icons"); }
    }
    // hairline: the Sound box (rows 0-1) — its top edge, and the separator between its rows
    let (bx, by) = (e(BOX_X), e(row_top(0) + GAP));
    let mx = e(BOX_X + BOX_W / 2);
    if px(mx, by) != hair || px(mx, by - 1) != face || px(mx, by + 1) != fill { return Err("hairline"); }
    let sy = e(row_top(1));
    if px(e(LABEL_X) + 2, sy) != hair || px(bx + 2, sy) != fill { return Err("hairline"); }
    // round: the box's corner and the selected (General) pill's corner are the ground under them
    if px(bx, by) != face || px(e(PILL_X), e(pill_y(0, 0))) != sb { return Err("round"); }
    Ok(face)
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
    let add = users_add(T, "tmp-pw1", "tmp-pw1", false);
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
    selftest_prefsui();
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
        if ns != crate::prefs::NS { return; } if k.starts_with("appearance.") { super::appearance::mark_dirty(); } if k.starts_with("accessibility.") { A11Y_DIRTY.store(true, Ordering::Release); } if k.starts_with("keyboard.") { KB_DIRTY.store(true, Ordering::Release); } // KEYMAPS (B704): another client chose a layout. A11YPANE (B655): the service pass re-reads and re-latches. APPEARANCE (B408): another client chose
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
    if BRIGHT_PERSIST_AT.load(Ordering::Acquire) != 0 { fresh.bright = old.bright; } // BRIGHTSLIDER M2: a slider move not yet stored is newer than the store
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

// ── BRIGHTSLIDER M2 (rmbp-ledger B377, R89) — THE SLIDER IS THE BRIGHTNESS ───────────────────────────────────────
// Flight 23: a press wrote the panel, then blocked ~1.3 s in the PrefSet (`[lag] click→shown … wm=1471.4`) BEFORE the
// repaint — the knob stayed where it was while the glass changed — and the track was quantised to 16 steps. Now the
// track pixel maps linearly onto the register (`backlight::raw_for_pos`), the write and its readback come first, the
// knob is repainted from the readback in the same act, and Principia's store write is debounced
// [`BRIGHT_PERSIST_MS`] onto the service pass (one store write per burst of clicks).

/// When the slider's store write is due (`arch::ms`, 0 = none pending).
static BRIGHT_PERSIST_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The debounce between a slider move and its store write.
pub const BRIGHT_PERSIST_MS: u64 = 750;
/// The register value the Brightness knob was last painted at (`u32::MAX` = never painted).
static PAINTED_RAW: AtomicU32 = AtomicU32::new(u32::MAX);

fn bright_persist_later() {
    BRIGHT_PERSIST_AT.store(crate::arch::ms().saturating_add(BRIGHT_PERSIST_MS).max(1), Ordering::Release);
}

/// A press on the Brightness track at logical `cx`: position → register (linear), write + readback, repaint, witness.
/// `[backlight] slider click pos=<pct>% -> reg=<n> readback=<n> slider=<pct>% sync=<ok|bad>`
fn bright_click(cx: usize) {
    let pos = cx.saturating_sub(TRACK_X).min(TRACK_W);
    let max = crate::video::backlight::max_now();
    let a = crate::video::backlight::set_raw_via(crate::video::backlight::raw_for_pos(pos, TRACK_W, max), "slider");
    CUR.lock().bright = a.level;
    repaint();
    let painted = PAINTED_RAW.load(Ordering::Relaxed);
    let sync = a.readback == Some(a.reg) && (!is_open() || painted == a.reg);
    serial_println!(
        "[backlight] slider click pos={}% -> reg={} readback={} slider={}% sync={}",
        (pos * 100 + TRACK_W / 2) / TRACK_W,
        a.reg, a.readback.map(|r| r as i64).unwrap_or(-1), crate::video::backlight::pct_of(if painted == u32::MAX { a.reg } else { painted }, a.max),
        if sync { "ok" } else { "bad" }
    );
    say("brightness", &alloc::format!("{}", a.level), a.on);
    bright_persist_later();
}

/// Left/Right on the Brightness control: one notch along the same linear scale as the keys (`backlight::step_raw`).
fn bright_key(up: bool) {
    let max = crate::video::backlight::max_now();
    let Some(l) = crate::video::backlight::step_raw(crate::video::backlight::cur_raw(), max, up) else { return };
    let a = crate::video::backlight::set_raw_via(crate::video::backlight::raw_for(l, max), "slider-key");
    CUR.lock().bright = a.level;
    repaint();
    say("brightness", &alloc::format!("{}", a.level), a.on);
    bright_persist_later();
}

/// `tests brightstep`: does the knob show register value `reg`? With the window open: repaint and compare what was
/// painted; closed: the linear mapping round-trips `reg` through a track position within one pixel's worth.
pub fn slider_sync(reg: u32) -> bool {
    let max = crate::video::backlight::panel_range();
    if is_open() {
        repaint();
        return PAINTED_RAW.load(Ordering::Relaxed) == reg;
    }
    let back = crate::video::backlight::raw_for_pos(crate::video::backlight::pos_for_raw(reg, max, TRACK_W), TRACK_W, max);
    back.abs_diff(reg) <= max / TRACK_W as u32 + 1
}

// ── PREFSUI (rmbp-ledger B389, R93) — A SLIDER DRAGS ─────────────────────────────────────────────────────────────
// Flight 24: `[backlight] slider click pos=16%` 13:12:14, `pos=35%` 13:12:16 — a drag was a run of presses, because
// the window saw only the press edge. Now a press on a slider track CAPTURES the pointer (`video::capture`): every
// motion sample while the button is held moves the knob and applies the value LIVE (the backlight follows the
// hand; the volume and the idle blank likewise), paced at [`DRAG_PACE_MS`]; the release applies the last position
// and COMMITS — one `[settings] <name>=<v>` line, one store write (brightness: BRIGHTSLIDER's debounce), and
// `[settings] slider drag key=<k> samples=<n> ms=<n> from=<v> to=<v>`.

/// The least time between two live applies of a drag (the release always applies the last position).
pub const DRAG_PACE_MS: u64 = 16;

#[derive(Clone, Copy)]
struct Drag {
    /// The control (0 brightness, 1 volume, 3 idle minutes).
    ctrl: usize,
    t0: u64,
    samples: u32,
    from: i64,
    /// The last logical x applied, and when.
    cx: usize,
    at: u64,
    /// The latest logical x seen (applied or paced out).
    want: usize,
}

static DRAG: crate::sync::Mutex<Option<Drag>> = crate::sync::Mutex::new(None);
/// Completed drags (the witness reads it).
static DRAGS: AtomicU32 = AtomicU32::new(0);

/// The control's value as the drag line prints it: brightness %, volume /16, idle minutes.
fn drag_value(ctrl: usize) -> i64 {
    match ctrl {
        0 => { let (r, m) = (crate::video::backlight::cur_raw(), crate::video::backlight::panel_range()); crate::video::backlight::pct_of(r.min(m), m) as i64 }
        1 => CUR.lock().vol as i64,
        A11Y_CURSOR_CTRL => A11Y_CURSOR_Q.load(Ordering::Relaxed) as i64, // A11YPANE (B655): quarters
        _ => CUR.lock().idle_min as i64,
    }
}

/// Apply control `ctrl` at logical `cx` LIVE: the device follows, the knob repaints, nothing is printed or stored.
fn drag_apply(ctrl: usize, cx: usize) {
    match ctrl {
        0 => {
            let pos = cx.saturating_sub(TRACK_X).min(TRACK_W);
            let max = crate::video::backlight::max_now();
            let a = crate::video::backlight::set_raw_via(crate::video::backlight::raw_for_pos(pos, TRACK_W, max), "slider-drag");
            CUR.lock().bright = a.level;
        }
        1 => { let v = slider_at(cx, 16) as u8; let mut c = CUR.lock(); if c.vol == v && !c.mute { return; } c.vol = v; c.mute = false; drop(c); apply_volume(v, false); }
        A11Y_CURSOR_CTRL => { let q = a11y_cursor_at(cx); if A11Y_CURSOR_Q.load(Ordering::Relaxed) == q { return; } a11y_set(2, q as usize, false); return; } // A11YPANE (B655): live, `a11y_set` repaints
        _ => { let m = IDLE_STEPS[slider_at(cx, IDLE_STEPS.len() - 1)]; let mut c = CUR.lock(); if c.idle_min == m { return; } c.idle_min = m; drop(c); apply_idle(m); }
    }
    repaint();
}

/// A press on slider `ctrl` at logical `cx`: apply it (brightness: [`bright_click`] already did) and capture.
fn drag_begin(ctrl: usize, cx: usize) {
    let now = crate::arch::ms();
    let from = drag_value(ctrl);
    if ctrl != 0 { drag_apply(ctrl, cx); }
    *DRAG.lock() = Some(Drag { ctrl, t0: now, samples: 0, from, cx, at: now, want: cx });
    super::capture::begin(drag_motion, drag_release);
}

/// A panel x as this window's logical x (left of the window = 0; the slider clamps the rest).
fn logical_x(x: i32) -> Option<usize> {
    let info = wm::info(WIN.load(Ordering::Relaxed))?;
    let sc = info.scale.max(1);
    Some(super::metrics::to_logical((x.max(0) as usize).saturating_sub(info.x) / sc))
}

/// The capture's motion: one more sample; applied when [`DRAG_PACE_MS`] has passed since the last apply.
fn drag_motion(x: i32, _y: i32) {
    let Some(cx) = logical_x(x) else { return };
    drag_step(cx, false);
}

fn drag_step(cx: usize, force: bool) {
    let now = crate::arch::ms();
    let go = {
        let mut g = DRAG.lock();
        let Some(d) = g.as_mut() else { return };
        d.samples += 1;
        d.want = cx;
        if cx == d.cx || (!force && now.saturating_sub(d.at) < DRAG_PACE_MS) { None } else { d.cx = cx; d.at = now; Some(d.ctrl) }
    };
    if let Some(c) = go { drag_apply(c, cx); }
}

/// The capture's release: the last position, then the commit (print + store) and the drag line.
fn drag_release(x: i32, _y: i32) {
    if let Some(cx) = logical_x(x) { if let Some(d) = DRAG.lock().as_mut() { d.want = cx; } }
    drag_finish();
}

fn drag_finish() {
    let Some(d) = DRAG.lock().take() else { return };
    if d.want != d.cx { drag_apply(d.ctrl, d.want); }
    let to = drag_value(d.ctrl);
    match d.ctrl {
        0 => { let l = CUR.lock().bright; say("brightness", &alloc::format!("{}", l), true); bright_persist_later(); }
        1 => { let v = CUR.lock().vol; say("volume", &alloc::format!("{}", v), true); persist(1); }
        A11Y_CURSOR_CTRL => { let q = A11Y_CURSOR_Q.load(Ordering::Relaxed); a11y_set(2, q as usize, true); } // A11YPANE (B655): the commit prints and latches the store write
        _ => { let m = CUR.lock().idle_min; say("idle_min", &alloc::format!("{}", m), true); persist(3); }
    }
    DRAGS.fetch_add(1, Ordering::Relaxed);
    serial_println!("[settings] slider drag key={} samples={} ms={} from={} to={}", NAMES.get(d.ctrl).copied().unwrap_or(crate::prefs::key::A11Y_CURSOR_SIZE), d.samples, crate::arch::ms().saturating_sub(d.t0), d.from, to);
}

/// `tests settings` leg: drive a drag on the Blank-screen slider THROUGH the capture seam (press, two motion
/// samples at panel coordinates, release) and read the value back; restores the operator's value. `"ok"` / why not.
#[cfg(feature = "witness")]
fn drag_selftest() -> &'static str {
    let before = CUR.lock().idle_min;
    let was_open = is_open();
    if !was_open && open().is_err() { return "no-window"; }
    let Some(info) = wm::info(WIN.load(Ordering::Relaxed)) else { return "no-window" };
    let px = |cx: usize| (info.x + super::metrics::size(cx) * info.scale.max(1)) as i32;
    let d0 = DRAGS.load(Ordering::Relaxed);
    drag_begin(3, TRACK_X);
    let a = CUR.lock().idle_min;
    super::capture::motion(px(TRACK_X + TRACK_W / 2), 0);
    for _ in 0..3 { let t = crate::arch::ms(); while crate::arch::ms() < t + DRAG_PACE_MS + 1 { core::hint::spin_loop(); } }
    super::capture::motion(px(TRACK_X + TRACK_W), 0);
    super::capture::release(px(TRACK_X + TRACK_W), 0);
    let b = CUR.lock().idle_min;
    let ok = a == IDLE_STEPS[0] && b == IDLE_STEPS[IDLE_STEPS.len() - 1] && DRAGS.load(Ordering::Relaxed) == d0 + 1 && !super::capture::held();
    set(3, idle_index(before));
    if !was_open { close(); }
    if ok { "ok" } else { "no" }
}

// ── PREFSUI (rmbp-ledger B389, R91) — the Login Items tab ───────────────────────────────────────────────────────
// Row 0: what it is. Rows 1..=LI_ROWS: the list in launch order, each with Up / Down / Remove. Row 9: Add — a
// chooser over the installed programs not yet in the list (< name >) and the Add button. Every edit is
// `loginitems::edit` (one latched store write of `system.login.items`, the key the dock menu's Open at Login edits).

const LI_ROWS: usize = 7;
const LI_UP_X: usize = PANE_X + 230;
const LI_DOWN_X: usize = LI_UP_X + BTN_W + 10;
const LI_DEL_X: usize = LI_DOWN_X + BTN_W + 10;
const LI_ADD_ROW: usize = 9;
/// The Add row's "previous" button (the chooser's `<`).
const LI_PREV_X: usize = PANE_X + 80;

/// The installed programs not yet in the list (the Add chooser's choices).
fn li_candidates() -> Vec<&'static str> {
    let l = super::loginitems::items();
    super::loginitems::installed().into_iter().filter(|n| !prefs_core::login::contains(&l, n)).collect()
}

fn paint_login(st: &mut State) {
    let l = super::loginitems::items();
    txt2(st, LABEL_X, 0, &alloc::format!("Open at login ({}), in this order:", l.len()));
    groups(st, &[(1, l.len().clamp(1, LI_ROWS)), (LI_ADD_ROW, LI_ADD_ROW)]);
    if l.is_empty() { txt2(st, LABEL_X, 1, "None: nothing opens itself at login."); }
    for (k, n) in l.iter().take(LI_ROWS).enumerate() {
        txt(st, LABEL_X, 1 + k, n);
        btn(st, 1 + k, LI_UP_X, "Up");
        btn(st, 1 + k, LI_DOWN_X, "Down");
        btn(st, 1 + k, LI_DEL_X, "Remove");
    }
    if l.len() > LI_ROWS { txt2(st, LABEL_X, 1 + LI_ROWS, &alloc::format!("+{} more", l.len() - LI_ROWS)); }
    let c = li_candidates();
    txt(st, LABEL_X, LI_ADD_ROW, "Add");
    if c.is_empty() { txt2(st, TRACK_X, LI_ADD_ROW, "Every program is in the list."); return; }
    let i = st.li_add % c.len();
    btn(st, LI_ADD_ROW, LI_PREV_X, "<");
    txt(st, LI_UP_X - 70, LI_ADD_ROW, c[i]);
    btn(st, LI_ADD_ROW, LI_DOWN_X, ">");
    btn_f(st, LI_ADD_ROW, LI_DEL_X, "Add", true, false);
}

fn press_login(row: usize, cx: usize) {
    let in_b = |x0: usize| cx >= x0 && cx < x0 + BTN_W;
    if (1..=LI_ROWS).contains(&row) {
        let l = super::loginitems::items();
        let Some(n) = l.get(row - 1) else { return };
        let op = if in_b(LI_UP_X) { "up" } else if in_b(LI_DOWN_X) { "down" } else if in_b(LI_DEL_X) { "remove" } else { return };
        super::loginitems::edit(op, n, "settings");
    } else if row == LI_ADD_ROW {
        let c = li_candidates();
        if c.is_empty() { return; }
        let mut g = STATE.lock();
        let Some(st) = g.as_mut() else { return };
        let i = st.li_add % c.len();
        if in_b(LI_PREV_X) { st.li_add = (i + c.len() - 1) % c.len(); }
        else if in_b(LI_DOWN_X) { st.li_add = (i + 1) % c.len(); }
        else if in_b(LI_DEL_X) { drop(g); super::loginitems::edit("add", c[i], "settings"); }
    } else { return; }
    repaint();
}

// ── PREFSUI (rmbp-ledger B389, R93) — the Resolution dropdown ───────────────────────────────────────────────────
// "i was seeing if i could change the monitor resolution. i guess we need to put adding a dropdown of the supported
// resolutions?" The display path sets ONE panel mode (the native one: after the Kepler takeover the GOP's list is
// gone and the panel runs 2880x1800); what a user can choose is the UI scale, so the entries are
// `prefs_core::modes::modes` — the native mode at each scale, a scaled one captioned `Looks like WxH`. A choice is
// latched in the router and applied by the service pass ([`mode_service`]): `dpi::set_live`, the faces re-size
// (`text::rescale`), every window repaints, this window re-opens at the new scale, and `system.display.mode` is
// stored. `[settings] display mode=<WxH> looks_like=<WxH> scale=<s> applied=<0|1> via=<settings|login>`.

/// A chosen entry owed to the service pass (index + 1; 0 = none).
static MODE_APPLY: AtomicU32 = AtomicU32::new(0);

/// The dropdown's entries for the live panel (empty while the panel is busy or unknown).
fn display_modes() -> Vec<prefs_core::modes::Mode> {
    let (w, h) = crate::video::panel_info_nonblocking().map_or((0, 0), |i| (i.width as u32, i.height as u32));
    prefs_core::modes::modes(w, h, super::dpi::native_s2())
}

/// The entry the live scale is.
fn mode_cur(ms: &[prefs_core::modes::Mode]) -> usize {
    let s2 = super::dpi::s2();
    ms.iter().position(|m| m.s2 == s2).unwrap_or_else(|| prefs_core::modes::default_index(ms))
}

fn mode_caption(m: &prefs_core::modes::Mode) -> String {
    alloc::format!("{} ({}x)", m.label(), super::dpi::scale_str(m.s2))
}

/// The closed popup: the kit's button across the row with the current entry and a chevron; the panel's own mode below.
fn mode_field(st: &mut State) {
    let ms = display_modes();
    let (y, w) = (mid_y(2, BTN_H), CTRL_R - TRACK_X);
    let c = cs(st, TRACK_X, y, w, BTN_H, false);
    {
        let mut s = sf(st);
        draw::push_button(&mut s, e(TRACK_X), e(y), ew(TRACK_X, w), b"", false, c);
        // The chevron: two strokes, down.
        let (cx, cy, a) = (e(CTRL_R - 14), e(y + BTN_H / 2), dp(4));
        let sw = dp(2).max(1) * draw::SUB * 3 / 4;
        let ink = theme::ink_secondary();
        draw::line_aa(&mut s, (cx - a) * draw::SUB, (cy - a / 2) * draw::SUB, cx * draw::SUB, (cy + a / 2) * draw::SUB, sw, ink);
        draw::line_aa(&mut s, cx * draw::SUB, (cy + a / 2) * draw::SUB, (cx + a) * draw::SUB, (cy - a / 2) * draw::SUB, sw, ink);
    }
    let t = match ms.get(mode_cur(&ms)) { Some(m) => mode_caption(m), None => String::from("panel busy") };
    txt(st, TRACK_X + 10, 2, &t);
    // PREFSUI M6 (seat): the panel's own mode, said once, so no one reads a scale choice as the panel changing.
    txt(st, LABEL_X, 8, "Panel");
    let native = match ms.first() { Some(m) => alloc::format!("{}x{} native (a choice above is a scale)", m.w, m.h), None => String::from("panel busy") };
    txt2(st, TRACK_X, 8, &native);
}

/// The open popup: a raised overlay under the field (shadowed, rounded, hairline-edged), one row per entry, the
/// current one an accent pill, the hovered one washed.
fn mode_list(st: &mut State) {
    let ms = display_modes();
    let cur = mode_cur(&ms);
    let n = ms.len().min(ROWS - 3);
    if n == 0 { return; }
    let (x, y, w) = (TRACK_X, row_top(3), CTRL_R - TRACK_X);
    {
        let mut s = sf(st);
        let r = Rect::new(e(x), e(y), ew(x, w), ew(y, n * ROW_H));
        let rad = dp(draw::DIMS.group_radius);
        draw::shadow_rrect(&mut s, r.offset(0, dp(2)), rad, dp(12), theme::shadow_window());
        draw::fill_rrect(&mut s, r, rad, theme::group_fill());
        draw::stroke_rrect(&mut s, r, rad, 1, Ink::argb(theme::control_edge()));
    }
    for (k, m) in ms.iter().enumerate().take(n) {
        let ry = row_top(3 + k);
        let c = cs(st, x, ry, w, ROW_H, false);
        let on = k == cur;
        if on || c.hover {
            let (px, py) = (x + 6, ry + 6);
            let r = Rect::new(e(px), e(py), ew(px, w - 12), ew(py, ROW_H - 12));
            let ink = if on { Ink::solid(theme::accent()) } else { Ink::argb(theme::hairline()) };
            draw::fill_rrect(&mut sf(st), r, dp(draw::DIMS.row_radius), ink);
        }
        txt_ink(st, x + 14, 3 + k, &mode_caption(m), if on { theme::ink_on_accent() } else { theme::text_primary() });
    }
}

/// A press on the Display tab while the dropdown is open (any press closes it; an entry is chosen) or on its
/// field (opens it). `true` when consumed.
fn mode_press(row: usize, cx: usize) -> bool {
    let open = STATE.lock().as_ref().map(|s| s.modes_open).unwrap_or(false);
    if open {
        if let Some(s) = STATE.lock().as_mut() { s.modes_open = false; }
        let n = display_modes().len();
        if cx >= TRACK_X && row >= 3 && row < 3 + n { MODE_APPLY.store((row - 3 + 1) as u32, Ordering::Release); serial_println!("[settings] display mode chosen entry={} of={}", row - 3, n); }
        repaint();
        return true;
    }
    if row == 2 && cx >= TRACK_X {
        if let Some(s) = STATE.lock().as_mut() { s.modes_open = true; s.strip = false; }
        repaint();
        return true;
    }
    false
}

/// Apply entry `i` LIVE; `store` = persist it as `system.display.mode`. Returns whether the scale moved.
fn apply_mode(i: usize, via: &str, store: bool) -> bool {
    let ms = display_modes();
    let Some(m) = ms.get(i).copied() else { return false };
    let (before, after) = super::dpi::set_live(m.s2);
    let moved = before != after;
    if moved {
        super::text::rescale();
        let _ = wm::damage_intersecting(0, 0, 1 << 16, 1 << 16);
    }
    if store {
        if m.default { let _ = crate::prefs_client::pref_set(crate::prefs::NS, crate::prefs::key::DISPLAY_MODE, crate::prefs::PrefValue::Str(String::new())); }
        else { crate::prefs_client::sys_set(crate::prefs::key::DISPLAY_MODE, crate::prefs::PrefValue::Str(m.value())); }
    }
    serial_println!("[settings] display mode={}x{} looks_like={} scale={} applied={} via={}", m.w, m.h, m.value(), super::dpi::scale_str(after), (super::dpi::s2() == m.s2) as u8, via);
    moved
}

/// The service pass: a chosen entry is applied, stored, and this window re-opens at the new scale.
fn mode_service() {
    let k = MODE_APPLY.swap(0, Ordering::AcqRel);
    if k == 0 { return; }
    if apply_mode(k as usize - 1, "settings", true) && is_open() {
        close();
        if let Err(e) = open() { serial_println!("[settings] refuse reason={}", e); }
    }
}

/// The login's load: the user's stored mode (a looks-like size this panel offers) becomes the session's scale.
fn mode_at_login() {
    let Some(v) = crate::prefs_client::sys_text(crate::prefs::key::DISPLAY_MODE) else { return };
    let ms = display_modes();
    if let Some(i) = prefs_core::modes::index_of(&ms, &v) {
        if ms[i].s2 != super::dpi::s2() { apply_mode(i, "login", false); }
    }
}

/// PREFSUI (B389) — the `tests settings` leg: a drag through the capture seam, a login-items round trip, the
/// Resolution dropdown's entries. `:: PREFSUI: slider_drag=<ok|no|no-window> login_items=<n> modes=<n> -> PASS ::`.
#[cfg(feature = "witness")]
pub fn selftest_prefsui() {
    let drag = drag_selftest();
    let li = super::loginitems::selftest();
    let modes = display_modes().len();
    let ok = drag == "ok" && li.is_some() && modes >= 1;
    serial_println!(
        ":: PREFSUI: slider_drag={} login_items={} modes={} -> {} ::",
        drag, li.map(|n| n as i64).unwrap_or(-1), modes, if ok { "PASS" } else { "FAIL" }
    );
}

// ── DOCK2 (rmbp-ledger B394, MACPARITY row 25) — Desktop & Dock: the dock's edge and auto-hide on the General tab ─────
// `system.dock.position` (Bottom · Left · Right, the Pointer row's segments) and `system.dock.autohide` (the Mute row's
// box). A press applies LIVE through the dock's own cells (`dock::set_position` / `set_autohide`) and latches ONE store
// write, drained on the dock's service pass (never a bus write in the click router).

const DOCK_POS_NAMES: [&str; 3] = ["Bottom", "Left", "Right"];

fn paint_dock_rows(st: &mut State) {
    txt(st, LABEL_X, 6, "Dock position");
    let p = super::dock::position() as usize;
    seg(st, 6, &[DOCK_POS_NAMES[0].as_bytes(), DOCK_POS_NAMES[1].as_bytes(), DOCK_POS_NAMES[2].as_bytes()], p, false);
    txt(st, LABEL_X, 7, "Automatically hide the dock");
    tog(st, 7, TOG_X, super::dock::autohide(), false);
    // SETTINGSMAP (B673): `system.dock.pins`' row — the pinned set is edited on the dock itself (its tile menu, a drag);
    // the row says how many it holds.
    txt(st, LABEL_X, 8, "Dock pins");
    txt2(st, TRACK_X, 8, &alloc::format!("{} pinned (edited on the dock)", super::dock::pins_pinned()));
}

fn dock_rows_press(row: usize, cx: usize) {
    if row == 6 && cx >= TRACK_X && cx < TRACK_X + TRACK_W {
        let k = ((cx - TRACK_X) / (TRACK_W / 3)).min(2);
        let p = ["bottom", "left", "right"][k];
        let ok = super::dock::set_position(p, true);
        say("dock_position", p, ok);
    } else if row == 7 && cx >= TOG_X && cx < TOG_X + draw::DIMS.toggle_w { // SETTINGSCRAFT: the switch, right-aligned
        let on = !super::dock::autohide();
        super::dock::set_autohide(on, true);
        say("dock_autohide", if on { "1" } else { "0" }, true);
    } else {
        return;
    }
    repaint();
}

/// FIRSTUSER (R100): is `name` an administrator (the Users list's ` (admin)` tag)?
fn users_is_admin(name: &str) -> bool {
    #[cfg(feature = "login")]
    { crate::fs::users::role_of(name.as_bytes()) == Some(crate::fs::users::Role::Admin) }
    #[cfg(not(feature = "login"))]
    { let _ = name; false }
}

// ── APPEARANCE (rmbp-ledger B408, MACPARITY rows 21/22) — the Appearance tab ─────────────────────────────────────────
// Light · Dark · Auto (the Dock row's segment shape), the accent swatches, the highlight swatches (the first follows
// the accent), and a selection sample. A press latches the choice (`appearance::choose`); the settings service pass
// stores the key and repaints everything once (`[appearance] … via=settings repaint_ms=<n>`). Mouse only: the tab has
// no keyboard controls (Left/Right on the strip still switch tabs).

const AP_MODES: [&str; 3] = ["Light", "Dark", "Auto"];
const SW: usize = 20;
const SW_PITCH: usize = 26;

/// A colour swatch: a disc with a hairline edge; the chosen one ringed in the ink, a hovered one in the control edge.
fn swatch(st: &mut State, r: usize, k: usize, c: u32, on: bool) {
    let (x, y) = (TRACK_X + k * SW_PITCH, mid_y(r, SW));
    let state = cs(st, x, y, SW, SW, false);
    let mut s = sf(st);
    let rad = ew(x, SW) / 2;
    let (cx, cy) = (e(x) + rad, e(y) + rad);
    if on {
        draw::stroke_circle(&mut s, cx, cy, rad + dp(3), dp(2).max(1), theme::text_primary());
    } else if state.hover {
        draw::stroke_circle(&mut s, cx, cy, rad + dp(3), dp(1).max(1), Ink::argb(theme::control_edge()));
    }
    draw::fill_circle(&mut s, cx, cy, rad, c);
    draw::stroke_circle(&mut s, cx, cy, rad, 1, Ink::argb(theme::hairline()));
}

fn paint_appearance(st: &mut State) {
    use prefs_core::appearance as ap;
    groups(st, &[(0, 0), (2, 4)]);
    let m = super::appearance::mode().index();
    txt(st, LABEL_X, 0, "Appearance");
    seg(st, 0, &[AP_MODES[0].as_bytes(), AP_MODES[1].as_bytes(), AP_MODES[2].as_bytes()], m, false);
    txt2(st, VAL_X, 0, if theme::is_dark() { "Dark now" } else { "Light now" });
    let clock = match super::appearance::local_hour() { Some(hr) => alloc::format!("now {:02}h", hr), None => String::from("no clock: light") };
    txt2(st, LABEL_X, 1, &alloc::format!("Auto = dark {:02}:00-{:02}:00 by the RTC until NETCLOCK sets the time ({})", ap::AUTO_DARK_FROM, ap::AUTO_DARK_UNTIL, clock));
    txt(st, LABEL_X, 2, "Accent colour");
    let a = super::appearance::accent_ix();
    for k in 0..theme::ACCENTS.len() { swatch(st, 2, k, theme::ACCENTS[k], k == a); }
    txt2(st, VAL_X, 2, ap::ACCENTS[a.min(7)]);
    txt(st, LABEL_X, 3, "Highlight colour");
    let hr = super::appearance::highlight_raw();
    for k in 0..ap::HIGHLIGHTS.len() { swatch(st, 3, k, if k == 0 { theme::ACCENTS[a.min(7)] } else { theme::ACCENTS[k - 1] }, k == hr); }
    txt2(st, VAL_X, 3, super::appearance::highlight_name());
    txt(st, LABEL_X, 4, "Selection");
    {
        let y = row_top(4) + 10;
        let r = Rect::new(e(TRACK_X), e(y), ew(TRACK_X, TRACK_W), ew(y, ROW_H - 20));
        draw::fill_rrect(&mut sf(st), r, dp(draw::DIMS.row_radius), theme::selection());
    }
    txt(st, TRACK_X + 8, 4, "Selected text looks like this");
    txt2(st, LABEL_X, 5, "The accent marks the default button, the selection, the focus ring,");
    txt2(st, LABEL_X, 6, "the slider knob and the menu highlight; the highlight is text selection.");
    txt(st, LABEL_X, 7, "Look"); seg(st, 7, &[b"UnaOS", b"Softy"], super::look::current() as usize, false); // SOFTY2 (B641): the LOOK row — Principia's `system.look`, live (`look::choose` on the press row below)
}

fn press_appearance(row: usize, cx: usize) {
    use prefs_core::appearance as ap;
    if cx < TRACK_X { return; }
    let k = cx - TRACK_X;
    let (kind, i, key, name) = match row {
        0 if k < TRACK_W => { let i = (k / (TRACK_W / 3)).min(2); (0, i, "appearance_mode", ap::MODES[i]) }
        2 if k < ap::ACCENTS.len() * SW_PITCH => { let i = k / SW_PITCH; (1, i, "appearance_accent", ap::ACCENTS[i]) }
        3 if k < ap::HIGHLIGHTS.len() * SW_PITCH => { let i = k / SW_PITCH; (2, i, "appearance_highlight", ap::HIGHLIGHTS[i]) }
        7 if k < TRACK_W => { let i = (k / (TRACK_W / 2)).min(1); super::look::choose(i); say("system_look", ap::LOOKS[i], true); repaint(); return; } // SOFTY2 (B641): Look — UnaOS | Softy (stored and applied by `look::service`, off the click router)
        _ => return,
    };
    super::appearance::choose(kind, i);
    say(key, name, true);
}

// ── TRACKPADPANE (rmbp-ledger B412, MACPARITY row 16) — the Trackpad tab ───────────────────────────────────────────
// Row 0 Tracking speed (a slider 1..10 that DRAGS through PREFSUI's capture seam, applied live as one gain on the
// TPSPEED curve); rows 1-4 toggles: Tap to click (off), Natural scrolling (on), Secondary click with two fingers (on),
// Three-finger drag (off); row 5 says where the values go. Every change applies LIVE to the gesture stage
// (`drivers::ehci::tpgest`), prints `[settings] trackpad.<key>=<v> applied=<0|1>`, and latches ONE store write of
// Principia's `system.trackpad.*`, drained by [`tp_service`] (never a bus write in the click router). The rules
// (gain table, legacy `pointer.speed` mapping, the wheel sign) are `prefs_core::trackpad`.

/// The Trackpad tab's index on the strip.
pub const TP_TAB: usize = 6; // merge17: APPEARANCE holds 5

#[derive(Clone, Copy)]
struct TpVals { speed: u8, tap: bool, natural: bool, secondary: bool, three: bool }

static TPV: crate::sync::Mutex<TpVals> = crate::sync::Mutex::new(TpVals {
    speed: prefs_core::trackpad::SPEED_DEFAULT,
    tap: prefs_core::trackpad::TAP_DEFAULT,
    natural: prefs_core::trackpad::NATURAL_DEFAULT,
    secondary: true,
    three: prefs_core::trackpad::THREE_DRAG_DEFAULT,
});
/// Store writes owed: bit k = row k (0 speed, 1 tap, 2 natural, 3 secondary, 4 three-finger drag).
static TP_PERSIST: AtomicU32 = AtomicU32::new(0);
const TP_ROW_NAMES: [&str; 5] = ["Tap to click", "Natural scrolling", "Secondary click (two fingers)", "Three-finger drag", ""];

/// Hand the pane's values to the driver's gesture stage. `true` when a Wellspring stage exists on this build.
fn tp_apply(v: TpVals) -> bool {
    #[cfg(all(target_arch = "x86_64", feature = "ehcihid"))]
    {
        use crate::drivers::ehci::tpgest as g;
        g::set_speed(v.speed); g::set_tap(v.tap); g::set_natural(v.natural); g::set_secondary(v.secondary); g::set_three_drag(v.three);
        true
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "ehcihid")))]
    { let _ = v; false }
}

/// The values the driver holds now (`None` where no Wellspring stage is built).
fn tp_live() -> Option<TpVals> {
    #[cfg(all(target_arch = "x86_64", feature = "ehcihid"))]
    { let (speed, tap, natural, secondary, three) = crate::drivers::ehci::tpgest::get(); Some(TpVals { speed, tap, natural, secondary, three }) }
    #[cfg(not(all(target_arch = "x86_64", feature = "ehcihid")))]
    { None }
}

/// Login: read the five keys (a stored legacy `pointer.speed` stands in for an unset speed) and apply them.
fn tp_load(legacy_ptr: Option<u8>) {
    use crate::prefs::key; use crate::prefs_client::{sys_flag as flag, sys_int as int, sys_text as text};
    let t = &prefs_core::trackpad::SECONDARY_CHOICES;
    let mut v = *TPV.lock();
    v.speed = match int(key::TP_SPEED, 1, 10) { Some(s) => s as u8, None => legacy_ptr.map_or(prefs_core::trackpad::SPEED_DEFAULT, |p| prefs_core::trackpad::speed_from_legacy(p as i64)) };
    if let Some(x) = flag(key::TP_TAP) { v.tap = x; }
    if let Some(x) = flag(key::TP_NATURAL) { v.natural = x; }
    if let Some(x) = text(key::TP_SECONDARY).filter(|s| t.contains(&s.as_str())) { v.secondary = prefs_core::trackpad::secondary_on(&x); }
    if let Some(x) = flag(key::TP_THREE_DRAG) { v.three = x; }
    *TPV.lock() = v;
    let a = tp_apply(v);
    serial_println!(
        "[settings] trackpad loaded speed={} tap={} natural={} secondary={} three_drag={} applied={}",
        v.speed, v.tap as u8, v.natural as u8, if v.secondary { "two-finger" } else { "off" }, v.three as u8, a as u8
    );
}

/// Set row `row` (0 speed = `val`; 1..=4 toggles = `val != 0`), apply live, print, latch the store write, repaint.
fn tp_set(row: usize, val: usize, store: bool) {
    let v = {
        let mut g = TPV.lock();
        match row {
            0 => g.speed = prefs_core::trackpad::clamp_speed(val as i64),
            1 => g.tap = val != 0,
            2 => g.natural = val != 0,
            3 => g.secondary = val != 0,
            4 => g.three = val != 0,
            _ => return,
        }
        *g
    };
    let a = tp_apply(v);
    if store {
        let (k, s) = match row {
            0 => (prefs_core::trackpad::KEY_SPEED, alloc::format!("{}", v.speed)),
            1 => (prefs_core::trackpad::KEY_TAP, alloc::format!("{}", v.tap as u8)),
            2 => (prefs_core::trackpad::KEY_NATURAL, alloc::format!("{}", v.natural as u8)),
            3 => (prefs_core::trackpad::KEY_SECONDARY, String::from(if v.secondary { "two-finger" } else { "off" })),
            _ => (prefs_core::trackpad::KEY_THREE_DRAG, alloc::format!("{}", v.three as u8)),
        };
        say(k, &s, a);
        TP_PERSIST.fetch_or(1 << row, Ordering::AcqRel);
    }
    repaint();
}

/// The service pass: the owed store writes (one PrefSet per changed key).
fn tp_service() {
    let owed = TP_PERSIST.swap(0, Ordering::AcqRel);
    if owed == 0 { return; }
    use crate::prefs::{key, PrefValue as P}; use crate::prefs_client::sys_set as set_sys;
    let v = *TPV.lock();
    if owed & 1 != 0 { set_sys(key::TP_SPEED, P::Int(v.speed as i64)); }
    if owed & 2 != 0 { set_sys(key::TP_TAP, P::Bool(v.tap)); }
    if owed & 4 != 0 { set_sys(key::TP_NATURAL, P::Bool(v.natural)); }
    if owed & 8 != 0 { set_sys(key::TP_SECONDARY, P::Str(String::from(if v.secondary { "two-finger" } else { "off" }))); }
    if owed & 16 != 0 { set_sys(key::TP_THREE_DRAG, P::Bool(v.three)); }
}

fn paint_trackpad(st: &mut State) {
    let v = *TPV.lock();
    groups(st, &[(0, 4)]);
    txt(st, LABEL_X, 0, "Tracking speed");
    slider(st, 0, (v.speed - 1) as usize, 9, false);
    let g = prefs_core::trackpad::gain8(v.speed);
    txt2(st, VAL_X, 0, &alloc::format!("{}/10 x{}.{:02}", v.speed, g / 8, g % 8 * 100 / 8));
    let on = [v.tap, v.natural, v.secondary, v.three];
    for (k, &b) in on.iter().enumerate() {
        let r = 1 + k;
        txt(st, LABEL_X, r, TP_ROW_NAMES[k]);
        tog(st, r, TOG_X, b, false); // SETTINGSCRAFT: the kit's switch, right-aligned (a press anywhere on the row flips it)
    }
    txt2(st, LABEL_X, 5, if tp_live().is_some() { "Applied live to the internal trackpad." } else { "No Wellspring trackpad path on this build; stored only." });
    txt2(st, LABEL_X, 6, "Speed scales the pointer curve; its shape stays as flown.");
}

/// A press on the Trackpad tab: the speed slider captures; a toggle box (or its label) flips.
fn press_trackpad(row: usize, cx: usize) {
    if row == 0 {
        if cx + 6 >= TRACK_X && cx <= TRACK_X + TRACK_W + 6 { tp_drag_begin(cx); }
        return;
    }
    if (1..=4).contains(&row) {
        let v = *TPV.lock();
        let cur = [v.tap, v.natural, v.secondary, v.three][row - 1];
        tp_set(row, (!cur) as usize, true);
    }
}

#[derive(Clone, Copy)]
struct TpDrag { t0: u64, samples: u32, from: u8, at: u64, want: usize, cx: usize }
static TPD: crate::sync::Mutex<Option<TpDrag>> = crate::sync::Mutex::new(None);
static TP_DRAGS: AtomicU32 = AtomicU32::new(0);

fn tp_speed_at(cx: usize) -> usize { 1 + slider_at(cx, 9) }

fn tp_drag_begin(cx: usize) {
    let now = crate::arch::ms();
    let from = TPV.lock().speed;
    tp_set(0, tp_speed_at(cx), false);
    *TPD.lock() = Some(TpDrag { t0: now, samples: 0, from, at: now, want: cx, cx });
    super::capture::begin(tp_drag_motion, tp_drag_release);
}

fn tp_drag_motion(x: i32, _y: i32) {
    let Some(cx) = logical_x(x) else { return };
    let now = crate::arch::ms();
    let go = {
        let mut g = TPD.lock();
        let Some(d) = g.as_mut() else { return };
        d.samples += 1;
        d.want = cx;
        if cx == d.cx || now.saturating_sub(d.at) < DRAG_PACE_MS { false } else { d.cx = cx; d.at = now; true }
    };
    if go { tp_set(0, tp_speed_at(cx), false); }
}

fn tp_drag_release(x: i32, _y: i32) {
    let Some(mut d) = TPD.lock().take() else { return };
    if let Some(cx) = logical_x(x) { d.want = cx; }
    tp_set(0, tp_speed_at(d.want), true);
    TP_DRAGS.fetch_add(1, Ordering::Relaxed);
    serial_println!(
        "[settings] slider drag key=trackpad.speed samples={} ms={} from={} to={}",
        d.samples, crate::arch::ms().saturating_sub(d.t0), d.from, TPV.lock().speed
    );
}

/// `tests trackpad`: drive the speed slider THROUGH the capture seam (press at the left end, a motion sample to the
/// right end, release), flip each toggle and read every value back from the gesture stage; then restore the
/// operator's values (no store write is left behind: the restore re-latches the original values).
/// `:: TRACKPADPANE: speed=<n> tap=<0/1> natural=<0/1> secondary=two-finger three_drag=<0/1> applied=live -> PASS ::`
pub fn selftest_trackpad() {
    let before = *TPV.lock();
    let was_open = is_open();
    if !was_open && open().is_err() { serial_println!(":: TRACKPADPANE: window=none -> FAIL ::"); return; }
    let Some(info) = wm::info(WIN.load(Ordering::Relaxed)) else { serial_println!(":: TRACKPADPANE: window=none -> FAIL ::"); return; };
    let px = |cx: usize| (info.x + super::metrics::size(cx) * info.scale.max(1)) as i32;
    let d0 = TP_DRAGS.load(Ordering::Relaxed);
    tp_drag_begin(TRACK_X);
    let low = tp_live().map_or(TPV.lock().speed, |l| l.speed);
    let t = crate::arch::ms(); while crate::arch::ms() < t + DRAG_PACE_MS + 1 { core::hint::spin_loop(); }
    super::capture::motion(px(TRACK_X + TRACK_W), 0);
    super::capture::release(px(TRACK_X + TRACK_W), 0);
    let high = tp_live().map_or(TPV.lock().speed, |l| l.speed);
    let drag_ok = low == 1 && high == 10 && TP_DRAGS.load(Ordering::Relaxed) == d0 + 1 && !super::capture::held();
    // The toggles: flip each away from its value and read the stage back.
    let flips = [(1, !before.tap), (2, !before.natural), (3, !before.secondary), (4, !before.three)];
    let mut tog_ok = true;
    for &(r, want) in flips.iter() {
        tp_set(r, want as usize, false);
        if let Some(l) = tp_live() { tog_ok &= [l.tap, l.natural, l.secondary, l.three][r - 1] == want; }
    }
    // The gain law at the default speed is the flown curve, pixel for pixel.
    let mut res = 0; let curve_ok = (-40..=40).all(|d| prefs_core::trackpad::gain_step(d, prefs_core::trackpad::SPEED_DEFAULT, &mut res) == d && res == 0);
    // Restore the operator's values (and the store, if this run latched a write).
    tp_set(0, before.speed as usize, false);
    for &(r, _) in flips.iter() { tp_set(r, [before.tap, before.natural, before.secondary, before.three][r - 1] as usize, false); }
    TP_PERSIST.fetch_or(1, Ordering::AcqRel);
    if !was_open { close(); }
    let live = tp_live();
    let applied = match live { Some(_) => "live", None => "none" };
    let v = live.unwrap_or(before);
    let ok = drag_ok && tog_ok && curve_ok && live.is_some();
    serial_println!(
        ":: TRACKPADPANE: speed={} tap={} natural={} secondary={} three_drag={} applied={} -> {} :: drag={}->{} toggles={} curve_default=x1",
        v.speed, v.tap as u8, v.natural as u8, if v.secondary { "two-finger" } else { "off" }, v.three as u8, applied,
        if ok { "PASS" } else if live.is_none() { "SKIP reason=no-wellspring-stage" } else { "FAIL" },
        low, high, if tog_ok { "ok" } else { "bad" }
    );
}

// ── LAUNCHER (B417): open on the row a preference key names ──────────────────────────────────

/// `(tab << 8) | control` latched by [`request_open_at`]; `u32::MAX` = none.
static AT: AtomicU32 = AtomicU32::new(u32::MAX);
const AT_FIRST: usize = 0xFF;

/// The tab and control a `system.*` schema key lives on (`AT_FIRST` = the tab's first control).
pub fn row_of_key(key: &str) -> (usize, usize) {
    if let Some(r) = MAP.iter().find(|r| r.tab < TABS && r.keys.contains(&key)) { return (r.tab, r.ctrl); } // SETTINGSMAP (B673): the one table
    match key {
        "audio.amp_holdoff_ms" => (0, 1),
        k if k.starts_with("display.") => (2, AT_FIRST),
        k if k.starts_with("login.") => (4, AT_FIRST),
        k if k.starts_with("accessibility.") => (A11Y_TAB, AT_FIRST), // A11YPANE (B655)
        k if k.starts_with("decisions.") => (PRIV_TAB, AT_FIRST), // REASK (B681)
        _ => (0, AT_FIRST),
    }
}

/// Open Settings (latched for [`service`], router-safe) on the row `key` names. Returns `(tab, control)`.
pub fn request_open_at(key: &str) -> (usize, usize) {
    let (tab, ctrl) = row_of_key(key);
    AT.store(((tab as u32) << 8) | ctrl as u32, Ordering::Release);
    if let Some(mut c) = CUR.try_lock() {
        c.tab = tab as u8;
    }
    request_open();
    (tab, ctrl)
}

/// The opening selection on `tab`: the latched row when one is owed for this tab (focus on the row), else the
/// tab's first control with focus on the strip (the old open).
fn take_at(tab: usize) -> (usize, bool) {
    let first = tab_ctrls(tab).first().copied().unwrap_or(0);
    let a = AT.swap(u32::MAX, Ordering::AcqRel);
    if a != u32::MAX && (a >> 8) as usize == tab {
        let c = (a & 0xFF) as usize;
        if c != AT_FIRST && tab_ctrls(tab).contains(&c) {
            serial_println!("[settings] open at tab={} control={} (launcher)", tab, NAMES[c]);
            return (c, false);
        }
        return (first, false);
    }
    (first, true)
}

// ── NOTIFYPANE (rmbp-ledger B435, MACPARITY rows 24/26) — the Notifications tab ─────────────────────────────────────
// Row 0 Do Not Disturb (NOTIFY's cell, `notify::set_dnd`, moved here from General row 8); row 1 its schedule, from / until
// a local hour stepped by -/+ (equal = no schedule); row 2 the column heads; rows 3..=8 the apps NOTIFY has seen (its
// posts) and the stored stanzas: Allow, Banner/Center, Sound; row 9 the `settings/notify` link. Every change applies
// LIVE in `notifypane`'s cell and latches ONE store write drained on NOTIFY's service pass (never a bus call here).

/// The Notifications tab's index on the strip (FILETYPES precedes it; A11YPANE (B655) follows it; REASK's Privacy is last).
pub const NP_TAB: usize = 8;
const NP_ROW0: usize = 3;
const NP_APP_ROWS: usize = 6;
const NP_ALLOW_X: usize = PANE_X + 300;
const NP_STYLE_X: usize = PANE_X + 400;
const NP_SOUND_X: usize = PANE_X + 540;
const NP_STEP_W: usize = 28;
/// Row 1's -/+ boxes: from-, from+, until-, until+ (x of each box).
const NP_STEPS: [usize; 4] = [TRACK_X, TRACK_X + 90, TRACK_X + 150, TRACK_X + 240];

fn paint_notifypane(st: &mut State) {
    let np = super::notifypane::window();
    let dnd = super::notify::dnd();
    let rows = super::notifypane::rows();
    let shown = rows.len().min(NP_APP_ROWS);
    if shown > 0 { groups(st, &[(0, 1), (NP_ROW0, NP_ROW0 + shown - 1)]); } else { groups(st, &[(0, 1)]); }
    txt(st, LABEL_X, 0, "Do Not Disturb");
    tog(st, 0, TOG_X, dnd, false);
    let sched = super::notifypane::in_window_now();
    txt2(st, TRACK_X, 0, if dnd { "On" } else if sched { "On (scheduled)" } else { "Off" });
    txt(st, LABEL_X, 1, "Scheduled");
    let sy = mid_y(1, BTN_H);
    for (k, x) in NP_STEPS.iter().enumerate() { btn_at(st, *x, sy, NP_STEP_W, if k % 2 == 0 { "-" } else { "+" }, false, false); }
    txt(st, NP_STEPS[0] + NP_STEP_W + 8, 1, &alloc::format!("{:02}:00", np.0));
    txt(st, NP_STEPS[2] + NP_STEP_W + 8, 1, &alloc::format!("{:02}:00", np.1));
    txt2(st, NP_STEPS[3] + NP_STEP_W + 12, 1, if np.0 == np.1 { "no schedule" } else { "quiet hours" });
    txt2(st, LABEL_X, 2, "App");
    txt2(st, NP_ALLOW_X - 8, 2, "Allow");
    txt2(st, NP_STYLE_X, 2, "Style");
    txt2(st, NP_SOUND_X - 8, 2, "Sound");
    if rows.is_empty() { txt2(st, LABEL_X, NP_ROW0, "No app has posted a notification yet."); }
    for (k, (name, r)) in rows.iter().take(NP_APP_ROWS).enumerate() {
        let row = NP_ROW0 + k;
        txt(st, LABEL_X, row, name);
        check(st, row, NP_ALLOW_X, r.allow);
        btn(st, row, NP_STYLE_X, if r.center { "Center" } else { "Banner" });
        check(st, row, NP_SOUND_X, r.sound);
    }
    if rows.len() > NP_APP_ROWS { txt2(st, NP_SOUND_X + 40, NP_ROW0 + NP_APP_ROWS - 1, &alloc::format!("+{} more", rows.len() - NP_APP_ROWS)); }
}

fn press_notifypane(row: usize, cx: usize) {
    let on_box = |x: usize| cx >= x && cx < x + 24;
    match row {
        0 if cx >= TOG_X && cx < TOG_X + draw::DIMS.toggle_w => { // SETTINGSCRAFT: the switch, right-aligned
            let on = !super::notify::dnd();
            super::notify::set_dnd(on, true);
            say("notify_dnd", if on { "1" } else { "0" }, true);
        }
        1 => {
            let Some(k) = NP_STEPS.iter().position(|x| cx >= *x && cx < *x + NP_STEP_W) else { return };
            super::notifypane::step_window(k / 2, if k % 2 == 0 { -1 } else { 1 });
        }
        r if (NP_ROW0..NP_ROW0 + NP_APP_ROWS).contains(&r) => {
            let i = r - NP_ROW0;
            if on_box(NP_ALLOW_X) { super::notifypane::toggle(i, 0); }
            else if cx >= NP_STYLE_X && cx < NP_STYLE_X + BTN_W { super::notifypane::toggle(i, 1); }
            else if on_box(NP_SOUND_X) { super::notifypane::toggle(i, 2); }
            else { return; }
        }
        _ => return,
    }
    repaint();
}

// ── FILETYPES (rmbp-ledger B423, MACPARITY §16 B3 / row 29) — the File Types pane ────────────────────────────────────
// Be's FileTypes preference: every type the registry knows (`/system/filetypes`), its description, MIME, extensions,
// icon glyph and PREFERRED APP. A press on a type's row hands the type to the next registrant (the programs whose
// resources declare it) — an ordinary `una:preferred` write on the type's object, latched here and applied by the
// service pass (`assoc::view_service`), never in the click router. Row 9: Prev / Next page. The rows are a snapshot
// the service pass reads (`assoc::view`), so the paint does no VFS work.

/// The File Types tab's index: the LAST tab (APPEARANCE B408 holds index 5 at the fold; this one is then 6).
pub const FT_TAB: usize = 7; // merge18: NOTIFYPANE (B435) is the last tab (8)
const FT_ROWS: usize = 7; // SETTINGSMAP (B673): row 8 is the sidebar favorites row
const FT_FAV_ROW: usize = 8;
const FT_NAV_ROW: usize = 9;
const FT_PREV_X: usize = TRACK_X;
const FT_NEXT_X: usize = TRACK_X + 2 * BTN_W + 40;
static FT_PAGE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

fn paint_filetypes(st: &mut State) {
    // SETTINGSMAP (B673): `system.quarry.favorites`' row — Quarry's sidebar folders, added by a drag onto the sidebar.
    groups(st, &[(FT_FAV_ROW, FT_FAV_ROW)]);
    let favs = crate::prefs::text(crate::prefs::key::QUARRY_FAVORITES).map_or(0, |t| t.split(',').filter(|f| f.trim().starts_with('/')).count());
    txt(st, LABEL_X, FT_FAV_ROW, "Sidebar favorites");
    txt2(st, TRACK_X, FT_FAV_ROW, &alloc::format!("{} added (drag a folder onto Quarry's sidebar)", favs));
    let v = crate::fs::assoc::view();
    let preferred = v.iter().filter(|r| !r.preferred.is_empty()).count();
    txt2(st, LABEL_X, 0, &alloc::format!("File types ({}, {} with an app). Click a type to change its app:", v.len(), preferred));
    if v.is_empty() { crate::fs::assoc::owe_view(); txt2(st, LABEL_X, 1, "Reading /system/filetypes ..."); return; }
    let pages = v.len().div_ceil(FT_ROWS).max(1);
    let page = FT_PAGE.load(Ordering::Relaxed).min(pages - 1);
    let shown = v.len().saturating_sub(page * FT_ROWS).min(FT_ROWS);
    groups(st, &[(1, shown.max(1))]);
    let face = super::text::Face::Ui;
    let ch = super::metrics::lcell_h(face);
    let app_x = CTRL_R - 140;
    for (k, r) in v.iter().skip(page * FT_ROWS).take(FT_ROWS).enumerate() {
        let row = 1 + k;
        // A type's row is one press target (it hands the type to its next app): it lights under the pointer.
        let c = cs(st, BOX_X, row_top(row), BOX_W, ROW_H, false);
        if c.hover || c.pressed { row_wash(st, row); }
        let t = 20;
        tab_icon(&mut sf(st), FT_TAB, e(LABEL_X), e(mid_y(row, t)), ew(LABEL_X, t), None);
        let app = if r.preferred.is_empty() { String::from("(no app)") } else { r.preferred_name.clone() };
        let ext = if r.extensions.is_empty() { String::new() } else { alloc::format!(" .{}", r.extensions.replace(", ", " .")) };
        let ty = row_top(row) + (ROW_H - ch) / 2;
        let pen = text_at(st, LABEL_X + t + 10, ty, app_x - 8, &r.description, theme::text_primary(), false, face);
        text_at(st, pen + 8, ty, app_x - 8, &alloc::format!("{}{}", r.mime, ext), theme::ink_secondary(), false, face);
        text_at(st, app_x, ty, CTRL_R, &alloc::format!("{} ({})", app, r.registrants), theme::ink_secondary(), false, face);
    }
    btn(st, FT_NAV_ROW, FT_PREV_X, "< Prev");
    txt2(st, FT_PREV_X + BTN_W + 14, FT_NAV_ROW, &alloc::format!("{}/{}", page + 1, pages));
    btn(st, FT_NAV_ROW, FT_NEXT_X, "Next >");
}

fn press_filetypes(row: usize, cx: usize) {
    let in_b = |x0: usize| cx >= x0 && cx < x0 + BTN_W;
    let n = crate::fs::assoc::view().len();
    let pages = n.div_ceil(FT_ROWS).max(1);
    let page = FT_PAGE.load(Ordering::Relaxed).min(pages - 1);
    if (1..=FT_ROWS).contains(&row) {
        let i = page * FT_ROWS + row - 1;
        if i < n { crate::fs::assoc::owe_cycle(i); }
    } else if row == FT_NAV_ROW {
        if in_b(FT_PREV_X) { FT_PAGE.store((page + pages - 1) % pages, Ordering::Relaxed); }
        else if in_b(FT_NEXT_X) { FT_PAGE.store((page + 1) % pages, Ordering::Relaxed); }
        else { return; }
    } else { return; }
    repaint();
}

// ── A11YPANE (rmbp-ledger B655) — the Accessibility tab ──────────────────────────────────────────────────────────────
// DESIGN-LANGUAGE §2.6 (Reduce motion: every slide a one-frame cut), §2.3 (High contrast: `text.secondary` ->
// `text.primary`, `line.*` -> `line.control`), [S1] (text enlarged to 200 %). The four `system.accessibility.*` keys
// (PREFS-SCHEMA.md; `prefs::key::A11Y_*`): `reduce_motion` (bool), `increase_contrast` (bool), `cursor_size` (float
// 1.0..=4.0, halves on the slider), `text_size` (enum default / large / larger / largest). Rows 0-1 toggles, row 2 a
// slider that DRAGS through PREFSUI's capture seam (`Drag.ctrl` = [`A11Y_CURSOR_CTRL`]), row 3 a segmented control.
// The APPLIED state is four atomics the glass reads on its hot paths with no lock and no bus call: `motion::reduced()`
// asks [`reduce_motion`] for every glide; `cursor::block_scale` asks [`cursor_blocks`] for every sprite draw (the
// panel's block scale x the size, capped at `ui::SCALE_MAX`, the save-under's box); [`increase_contrast`] is the
// theme's to read (owed: theme.rs is not this arc's file). The text size goes through the face seam the faces already
// follow: a step writes `system.display.font_size` (its px, [`A11Y_TEXT_PX`]) and `video::text` restyles on its next
// poll at CSS px x the panel's ppi / 96; at login nothing is written (the store carries the px). Loaded with the
// login's load ([`a11y_load`]), followed on PrefChanged (`bus_changes_inner` marks [`A11Y_DIRTY`], the service pass
// re-reads), stored as ONE latched PrefSet per changed key drained by [`a11y_service`] (never a bus write in the click
// router). Wire: `[settings] accessibility.<key>=<v> applied=<0|1>`, `[settings] accessibility loaded …`.
// A11YFILTERS (rmbp-ledger B705): rows 4-5 — `cvd` (enum none / protan / deutan / tritan / grayscale, a segmented
// control) and `invert` (bool, a switch), applied to `blitter::a11yfilter`'s latch, which every writer to the scanout
// reads at its copy (BP-video-25); ZOOM (B713) rows 6-7, PERFHUD (B710) row 8, the help text row 9. The filter's cost line,
// `[a11y] filter=<mode> pass_us=<n> motion=<full|reduced> presents=<n> px=<n> why=<load|change|tick>`, is printed by
// [`a11y_cost_line`] at the login's load, on every stored change of row 0 or rows 4..=8, and every ten seconds while a filter is on.

use super::blitter::a11yfilter as af;

/// The Accessibility tab's index on the strip (REASK B681's Privacy follows it).
pub const A11Y_TAB: usize = 9;
/// The schema's `accessibility.text_size` spellings, in step order.
const A11Y_TEXT_NAMES: [&str; 4] = ["default", "large", "larger", "largest"];
/// The CSS px each text step writes to `system.display.font_size`: the body size (DESIGN-LANGUAGE §2.2, 13), x1.25,
/// x1.5, x2.0 ([S1]: text enlarged by at least 200 %). Every step is inside the schema's `9..=32`.
pub const A11Y_TEXT_PX: [i64; 4] = [13, 16, 20, 26];
/// The cursor size in QUARTERS (4 = x1.0 .. 16 = x4.0): the atomic's unit, so the float never lives on a paint path.
const A11Y_CURSOR_MIN_Q: u8 = 4;
const A11Y_CURSOR_MAX_Q: u8 = 16;
/// The slider's last position (0..=6 -> x1.0..=x4.0 by halves).
const A11Y_CURSOR_STEPS: usize = 6;
/// The `Drag.ctrl` of the cursor-size slider (past [`CONTROLS`]: not a [`NAMES`] control).
const A11Y_CURSOR_CTRL: usize = CONTROLS;

static A11Y_REDUCE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(draw::motion::REDUCE_MOTION);
static A11Y_CONTRAST: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static A11Y_CURSOR_Q: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(A11Y_CURSOR_MIN_Q);
static A11Y_TEXT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
/// Store writes owed: bit k = row k (0 reduce motion, 1 contrast, 2 cursor size, 3 text size, 4 cvd, 5 invert, 6 zoom, 7 zoom follow, 8 show performance).
static A11Y_PERSIST: AtomicU32 = AtomicU32::new(0);
/// ZOOM (B713): the magnifier's factor as stored (0 off, 2, 3, 4) and what it follows (0 the pointer, 1 the focus) —
/// the rows' latch; the lens (`video/zoom.rs`, x86 `wc`) is handed both by [`zoom_apply`].
static A11Y_ZOOM: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
static A11Y_FOLLOW: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
/// The zoom row's segments, in order, and the factor each one is.
const A11Y_ZOOM_SEGS: [&[u8]; 4] = [b"Off", b"2x", b"3x", b"4x"];
/// The schema's `accessibility.zoom_follow` spellings (row 5's segments: Pointer, Focus).
const A11Y_FOLLOW_NAMES: [&str; 2] = ["pointer", "focus"];

/// Segment `i` of the zoom row as a factor (0, 2, 3, 4). Pure.
fn a11y_zoom_of_seg(i: usize) -> u8 {
    match i { 0 => 0, 1 => 2, 2 => 3, _ => 4 }
}

/// A factor as the zoom row's segment. Pure.
fn a11y_zoom_seg(z: u8) -> usize {
    match z { 0 | 1 => 0, 2 => 1, 3 => 2, _ => 3 }
}

/// Hand the latched factor and follow to the lens (x86 `wc`; elsewhere the row stores the keys and no lens exists).
fn zoom_apply() {
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    super::zoom::set(A11Y_ZOOM.load(Ordering::Relaxed), A11Y_FOLLOW.load(Ordering::Relaxed));
}
/// A PrefChanged named an `accessibility.*` key: the service pass re-reads the four.
static A11Y_DIRTY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Reduce motion, as applied (`motion::reduced()` reads this for every glide).
#[inline]
pub fn reduce_motion() -> bool {
    A11Y_REDUCE.load(Ordering::Relaxed)
}

/// Increase contrast, as applied: the switch `theme::tok` asks to answer from `palette_core::contrast`'s token set in
/// place of the standard pairs (one table, both sets; no painter keeps its own high-contrast rule).
#[inline]
pub fn increase_contrast() -> bool {
    A11Y_CONTRAST.load(Ordering::Relaxed)
}

/// The cursor sprite's block scale for the panel's `base` scale: `base x size` rounded, held to `1..=ui::SCALE_MAX`
/// (the save-under buffer is sized for `SCALE_MAX`: a larger block would be refused by `draw_locked`). Pure.
#[inline]
pub fn cursor_blocks(base: usize) -> usize {
    ((base * A11Y_CURSOR_Q.load(Ordering::Relaxed) as usize + 2) / 4).clamp(1, crate::ui::SCALE_MAX)
}

/// A stored `cursor_size` as quarters: finite, held to 1.0..=4.0, rounded to the nearest quarter. Pure.
fn a11y_cursor_q_of(f: f64) -> u8 {
    let f = if f.is_finite() { f.clamp(1.0, 4.0) } else { 1.0 };
    ((f * 4.0 + 0.5) as u8).clamp(A11Y_CURSOR_MIN_Q, A11Y_CURSOR_MAX_Q)
}

/// Quarters as the pane prints them: `1.50`.
fn a11y_cursor_str(q: u8) -> String {
    alloc::format!("{}.{:02}", q / 4, (q % 4) as u32 * 25)
}

/// The slider position `cx` as quarters (halves: 4, 6, .. 16).
fn a11y_cursor_at(cx: usize) -> u8 {
    A11Y_CURSOR_MIN_Q + 2 * slider_at(cx, A11Y_CURSOR_STEPS) as u8
}

/// The text step whose px `font_size` is (`None` = a size no step writes: the Display tab's - / + set it).
fn a11y_text_step_of(px: i64) -> Option<usize> {
    A11Y_TEXT_PX.iter().position(|p| *p == px)
}

/// Read the four keys over the bus (the schema's default where unset), latch them for the glass. `via` = `login`
/// (the login's load) or `prefchanged` (another client wrote one); on `prefchanged` a moved text step is re-applied
/// (its px written) through the same latched write a press makes.
fn a11y_load(via: &str) {
    use crate::prefs::key; use crate::prefs_client::{sys_flag as flag, sys_get, sys_text as text};
    let rm = flag(key::A11Y_REDUCE_MOTION).unwrap_or(draw::motion::REDUCE_MOTION);
    let ic = flag(key::A11Y_INCREASE_CONTRAST).unwrap_or(false);
    let cq = sys_get(key::A11Y_CURSOR_SIZE).and_then(|v| v.as_float()).map_or(A11Y_CURSOR_MIN_Q, a11y_cursor_q_of);
    let ts = text(key::A11Y_TEXT_SIZE).and_then(|s| A11Y_TEXT_NAMES.iter().position(|n| *n == s)).unwrap_or(0) as u8;
    let cvd = text(key::A11Y_CVD).and_then(|s| af::mode_of(&s)).unwrap_or(0);
    let inv = flag(key::A11Y_INVERT).unwrap_or(false); let _ = super::perfhud::latch(flag(key::A11Y_SHOW_PERF).unwrap_or(false), via); // PERFHUD (B710)
    let (rm0, ic0, cq0, ts0) = (A11Y_REDUCE.swap(rm, Ordering::Relaxed), A11Y_CONTRAST.swap(ic, Ordering::Relaxed), A11Y_CURSOR_Q.swap(cq, Ordering::Relaxed), A11Y_TEXT.swap(ts, Ordering::Relaxed));
    let (cvd0, inv0) = (af::mode(), af::invert());
    af::set(cvd, inv); if cvd0 != cvd || inv0 != inv { a11y_filter_moved(); } // crawl A 6: the whole glass takes the new filter (BP-video-25)
    let moved = (rm0 != rm) as u8 + (ic0 != ic) as u8 + (cq0 != cq) as u8 + (ts0 != ts) as u8 + (cvd0 != cvd) as u8 + (inv0 != inv) as u8;
    if via != "login" && ts0 != ts { A11Y_PERSIST.fetch_or(1 << 3, Ordering::AcqRel); }
    let zm = crate::prefs_client::sys_int(key::A11Y_ZOOM, 0, 4).map_or(0, |v| a11y_zoom_of_seg(a11y_zoom_seg(v as u8))); // ZOOM (B713)
    let zf = text(key::A11Y_ZOOM_FOLLOW).and_then(|s| A11Y_FOLLOW_NAMES.iter().position(|n| *n == s)).unwrap_or(0) as u8;
    let zoom_owed = A11Y_PERSIST.load(Ordering::Acquire) & (3 << 6) != 0; if !zoom_owed { A11Y_ZOOM.store(zm, Ordering::Relaxed); } // crawl A 15: a chord's or a press's zoom write still owed is newer than the store, so the re-read leaves the lens alone
    if !zoom_owed { A11Y_FOLLOW.store(zf, Ordering::Relaxed); }
    if !zoom_owed { zoom_apply(); }
    if via == "login" || moved > 0 {
        serial_println!("[settings] accessibility loaded reduce_motion={} increase_contrast={} cursor_size=x{} text_size={} cvd={} invert={} moved={} via={}", rm as u8, ic as u8, a11y_cursor_str(cq), A11Y_TEXT_NAMES[ts as usize], af::NAMES[cvd], inv as u8, moved, via);
        a11y_cost_line(if via == "login" { "load" } else { "change" });
    }
}

/// A11YFILTERS (B705): the filter's wire line, draining the cost counters: `pass_us` = the transform's microseconds
/// per filtered window present since the last line (every scanout writer's transform summed, over the window
/// presents `present_band` counted), `px` the pixels filtered.
fn a11y_cost_line(why: &str) {
    let (p, px, us) = af::take_stats();
    serial_println!(
        "[a11y] filter={} pass_us={} motion={} presents={} px={} why={}",
        af::label(af::mode(), af::invert()), if p > 0 { us / p } else { 0 }, if draw::motion::reduced() { "reduced" } else { "full" }, p, px, why
    );
}

/// The cost line's clock: `arch::ms()` of the last periodic line.
static A11Y_COST_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The periodic cost line's interval while a filter is on (ms).
const A11Y_COST_MS: u64 = 10_000;

/// Set row `row` (0 / 1 / 5 / 8 switches = `val != 0`; 2 the cursor size in quarters; 3 the text step; 4 the cvd mode; 6 the zoom factor; 7 the follow), apply it to the latch
/// the glass reads, and when `store`: print the change and latch the store write. Repaints.
fn a11y_set(row: usize, val: usize, store: bool) {
    let (name, vtxt, applied) = match row {
        0 => { A11Y_REDUCE.store(val != 0, Ordering::Relaxed); ("reduce_motion", alloc::format!("{}", (val != 0) as u8), true) }
        1 => { A11Y_CONTRAST.store(val != 0, Ordering::Relaxed); let _ = super::theme::set_contrast(val != 0); /* CONTRAST (B664): the theme takes the set and bumps its epoch, so chrome, menus and the generated wallpaper redraw as an accent change does. ⚠ LINE-NEUTRAL append. */ ("increase_contrast", alloc::format!("{}", (val != 0) as u8), true) }
        2 => { let q = (val.min(255) as u8).clamp(A11Y_CURSOR_MIN_Q, A11Y_CURSOR_MAX_Q); A11Y_CURSOR_Q.store(q, Ordering::Relaxed); ("cursor_size", alloc::format!("x{}", a11y_cursor_str(q)), super::cursor::armed()) }
        3 => { let s = val.min(A11Y_TEXT_NAMES.len() - 1); A11Y_TEXT.store(s as u8, Ordering::Relaxed); ("text_size", alloc::format!("{} ({} px)", A11Y_TEXT_NAMES[s], A11Y_TEXT_PX[s]), true) }
        4 => { let m = val.min(af::MODES - 1); if m != af::mode() { af::set(m, af::invert()); a11y_filter_moved(); } ("cvd", String::from(af::NAMES[m]), true) }
        5 => { if (val != 0) != af::invert() { af::set(af::mode(), val != 0); a11y_filter_moved(); } ("invert", alloc::format!("{}", (val != 0) as u8), true) }
        6 => { let z = a11y_zoom_of_seg(a11y_zoom_seg(val.min(4) as u8)); A11Y_ZOOM.store(z, Ordering::Relaxed); zoom_apply(); ("zoom", alloc::format!("{}", z), cfg!(all(target_arch = "x86_64", feature = "wc"))) } // ZOOM (B713)
        7 => { let f = val.min(A11Y_FOLLOW_NAMES.len() - 1); A11Y_FOLLOW.store(f as u8, Ordering::Relaxed); zoom_apply(); ("zoom_follow", alloc::format!("{}", A11Y_FOLLOW_NAMES[f]), cfg!(all(target_arch = "x86_64", feature = "wc"))) }
        8 => { let _ = super::perfhud::apply(val != 0, "settings", false); /* crawl A 24: the store write is this row's bit 8, not perfhud's OWED too */ ("show_performance", alloc::format!("{}", (val != 0) as u8), cfg!(all(target_arch = "x86_64", feature = "wc"))) } // PERFHUD (B710): applied where the compositor paints it
        _ => return,
    };
    if store {
        say(&alloc::format!("accessibility.{}", name), &vtxt, applied);
        A11Y_PERSIST.fetch_or(1 << row, Ordering::AcqRel);
        if row == 0 || row >= 4 { a11y_cost_line("change"); }
    }
    repaint();
}

/// The service pass: a PrefChanged's re-read, then the owed store writes (one PrefSet per changed key; the text
/// step also writes its px to `system.display.font_size`, the seam `video::text` follows within a second).
fn a11y_service() {
    if af::active() {
        let now = crate::arch::ms();
        if now.saturating_sub(A11Y_COST_AT.load(Ordering::Relaxed)) >= A11Y_COST_MS {
            A11Y_COST_AT.store(now, Ordering::Relaxed);
            a11y_cost_line("tick");
        }
    }
    // ZOOM (B713): the lens's own pass (engage / disengage off the input band, owed presents, the `[zoom]` line), then
    // a chord's step taken into the row's latch and stored like a press — only once the login's load has run. crawl A 15:
    // BEFORE the PrefChanged re-read below, so a reload in the same pass cannot reset the lens to the stored value first.
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    {
        super::zoom::service();
        if LOAD_DONE.load(Ordering::Acquire) {
            if let Some((z, f)) = super::zoom::take_persist() {
                A11Y_ZOOM.store(z, Ordering::Relaxed);
                A11Y_FOLLOW.store(f, Ordering::Relaxed);
                say("accessibility.zoom", &alloc::format!("{}", z), true);
                A11Y_PERSIST.fetch_or(1 << 6, Ordering::AcqRel); // the fold: zoom is row 6
                if is_open() && cur_tab() == A11Y_TAB { repaint(); }
            }
        }
    }
    if A11Y_DIRTY.swap(false, Ordering::AcqRel) && LOAD_DONE.load(Ordering::Acquire) {
        a11y_load("prefchanged");
        if is_open() && cur_tab() == A11Y_TAB { repaint(); }
    }
    let owed = A11Y_PERSIST.swap(0, Ordering::AcqRel) | if super::perfhud::take_owed() { 1 << 8 } else { 0 }; // PERFHUD (B710): the chord's toggle is written back here too (row 8 at the fold)
    if owed == 0 { return; }
    use crate::prefs::{key, PrefValue as P}; use crate::prefs_client::sys_set as set_sys;
    if owed & 1 != 0 { set_sys(key::A11Y_REDUCE_MOTION, P::Bool(A11Y_REDUCE.load(Ordering::Relaxed))); }
    if owed & 2 != 0 { set_sys(key::A11Y_INCREASE_CONTRAST, P::Bool(A11Y_CONTRAST.load(Ordering::Relaxed))); }
    if owed & 4 != 0 { set_sys(key::A11Y_CURSOR_SIZE, P::Float(A11Y_CURSOR_Q.load(Ordering::Relaxed) as f64 / 4.0)); }
    if owed & 8 != 0 {
        let s = (A11Y_TEXT.load(Ordering::Relaxed) as usize).min(A11Y_TEXT_NAMES.len() - 1);
        set_sys(key::A11Y_TEXT_SIZE, P::Str(String::from(A11Y_TEXT_NAMES[s])));
        set_sys(key::FONT_SIZE, P::Int(A11Y_TEXT_PX[s]));
    }
    if owed & 16 != 0 { set_sys(key::A11Y_CVD, P::Str(String::from(af::NAMES[af::mode()]))); }
    if owed & 32 != 0 { set_sys(key::A11Y_INVERT, P::Bool(af::invert())); }
    if owed & 64 != 0 { set_sys(key::A11Y_ZOOM, P::Int(A11Y_ZOOM.load(Ordering::Relaxed) as i64)); } // ZOOM (B713)
    if owed & 128 != 0 {
        let f = (A11Y_FOLLOW.load(Ordering::Relaxed) as usize).min(A11Y_FOLLOW_NAMES.len() - 1);
        set_sys(key::A11Y_ZOOM_FOLLOW, P::Str(String::from(A11Y_FOLLOW_NAMES[f])));
    }
    if owed & 256 != 0 { set_sys(key::A11Y_SHOW_PERF, P::Bool(super::perfhud::visible())); } // PERFHUD (B710), row 8
}

fn paint_a11y(st: &mut State) {
    groups(st, &[(0, 1), (2, 3), (4, 5), (6, 7), (8, 8)]);
    let (rm, ic, q) = (reduce_motion(), increase_contrast(), A11Y_CURSOR_Q.load(Ordering::Relaxed));
    txt(st, LABEL_X, 0, "Reduce motion");
    tog(st, 0, TOG_X, rm, false); // SETTINGSCRAFT: the kit's switch, right-aligned (a press anywhere on the row flips it)
    txt2(st, TRACK_X, 0, if rm { "Slides are cuts" } else { "Slides ease over 160 ms" });
    txt(st, LABEL_X, 1, "Increase contrast");
    tog(st, 1, TOG_X, ic, false);
    txt2(st, TRACK_X, 1, if ic { "Secondary ink and lines at full strength" } else { "Standard contrast" });
    txt(st, LABEL_X, 2, "Cursor size");
    slider(st, 2, ((q - A11Y_CURSOR_MIN_Q) / 2) as usize, A11Y_CURSOR_STEPS, false);
    txt2(st, VAL_X, 2, &alloc::format!("x{} ({} blocks)", a11y_cursor_str(q), cursor_blocks(crate::ui::Metrics::panel().scale)));
    txt(st, LABEL_X, 3, "Text size");
    let px = font_size();
    let step = a11y_text_step_of(px).unwrap_or(A11Y_TEXT.load(Ordering::Relaxed) as usize).min(A11Y_TEXT_NAMES.len() - 1);
    seg(st, 3, &[b"Default", b"Large", b"Larger", b"Largest"], step, false);
    txt2(st, VAL_X, 3, &alloc::format!("{} px", px));
    let (m, inv) = (af::mode(), af::invert());
    txt(st, LABEL_X, 4, "Colour filter");
    seg(st, 4, &[b"None", b"Red", b"Green", b"Blue", b"Gray"], m, false);
    txt2(st, VAL_X, 4, A11Y_CVD_SAY[m]);
    txt(st, LABEL_X, 5, "Invert colours");
    tog(st, 5, TOG_X, inv, false);
    txt2(st, TRACK_X, 5, if inv { "Every colour on the glass inverted" } else { "Colours as drawn" });
    let (zm, zf) = (A11Y_ZOOM.load(Ordering::Relaxed), (A11Y_FOLLOW.load(Ordering::Relaxed) as usize).min(1)); // ZOOM (B713)
    txt(st, LABEL_X, 6, "Zoom");
    seg(st, 6, &A11Y_ZOOM_SEGS, a11y_zoom_seg(zm), false);
    txt2(st, VAL_X, 6, if zm == 0 { "Win+= zooms in" } else { "Win+- out, Win+0 off" });
    txt(st, LABEL_X, 7, "Zoom follows");
    seg(st, 7, &[b"Pointer", b"Focus"], zf, false);
    let hud = super::perfhud::visible(); // PERFHUD (B710): the overlay's switch
    txt(st, LABEL_X, 8, "Show performance");
    tog(st, 8, TOG_X, hud, false);
    txt2(st, TRACK_X, 8, if hud { "Frame times, latency and memory, top right" } else { "Off (Win+F12 toggles it)" });
    txt2(st, LABEL_X, 9, "Zoom magnifies around the pointer or focus; the colour filter and invert cover the whole glass."); // crawl A 3: ROWS = 10, so row 9 is the last painted row (row 10 clipped at WIN_H)
}

/// A11YFILTERS (B705): what row 4's value column says per mode (the segments name the cone each filter simulates).
const A11Y_CVD_SAY: [&str; 5] = ["Off", "Protanopia", "Deuteranopia", "Tritanopia", "Grayscale"];

/// A press on the Accessibility tab: a toggle row flips, the cursor slider captures (a drag), a text segment sets.
fn press_a11y(row: usize, cx: usize) {
    match row {
        0 => a11y_set(0, (!reduce_motion()) as usize, true),
        1 => a11y_set(1, (!increase_contrast()) as usize, true),
        2 => { if cx + 6 >= TRACK_X && cx <= TRACK_X + TRACK_W + 6 { drag_begin(A11Y_CURSOR_CTRL, cx); } }
        3 => { if cx >= TRACK_X && cx < TRACK_X + TRACK_W { a11y_set(3, ((cx - TRACK_X) / (TRACK_W / A11Y_TEXT_NAMES.len())).min(A11Y_TEXT_NAMES.len() - 1), true); } }
        4 => { if cx >= TRACK_X && cx < TRACK_X + TRACK_W { a11y_set(4, ((cx - TRACK_X) / (TRACK_W / af::MODES)).min(af::MODES - 1), true); } }
        5 => a11y_set(5, (!af::invert()) as usize, true),
        6 => { if cx >= TRACK_X && cx < TRACK_X + TRACK_W { a11y_set(6, a11y_zoom_of_seg((cx - TRACK_X) / (TRACK_W / A11Y_ZOOM_SEGS.len())) as usize, true); } } // ZOOM (B713)
        7 => { if cx >= TRACK_X && cx < TRACK_X + TRACK_W { a11y_set(7, ((cx - TRACK_X) / (TRACK_W / A11Y_FOLLOW_NAMES.len())).min(A11Y_FOLLOW_NAMES.len() - 1), true); } }
        8 => a11y_set(8, (!super::perfhud::visible()) as usize, true), // PERFHUD (B710)
        _ => {}
    }
}

/// `tests a11y`: the four keys are in the schema; the pane paints offscreen with its four controls; a glide under
/// Reduce motion is a one-frame cut (and eases without it); the contrast latch reads back; the cursor seam doubles
/// the blocks at x2 and holds `SCALE_MAX`; the text steps are the body size, rising, to 200 %, inside the font-size
/// range. Every latch is restored; nothing is written to the store.
/// `:: A11Y: keys=4 declared=ok pane=ok reduce_motion=snap(ok) contrast=ok cursor=ok(x2) text=ok -> PASS ::`
pub fn selftest_a11y() {
    use crate::prefs::key;
    let keys = [key::A11Y_REDUCE_MOTION, key::A11Y_INCREASE_CONTRAST, key::A11Y_CURSOR_SIZE, key::A11Y_TEXT_SIZE];
    let declared = keys.iter().filter(|k| prefs_core::schema::lookup(crate::prefs::NS, k).is_some()).count();
    // The pane, painted offscreen at the panel's scale: the card, the search field and the four controls record hits.
    let pane = {
        let (pw, ph) = (super::metrics::size(WIN_W), super::metrics::size(WIN_H));
        let mut surf: Vec<u32> = Vec::new();
        if surf.try_reserve_exact(pw * ph).is_err() { false } else {
            surf.resize(pw * ph, 0);
            let mut st = State::new(WIN_W, WIN_H, surf, 0, true, 0);
            let mut v = CUR.lock().clone();
            v.tab = A11Y_TAB as u8;
            paint(&mut st, &v);
            st.hits.len() >= 6 && TAB_NAMES[A11Y_TAB] == "Accessibility" && col_list("access").1 == 1 && NP_TAB + 1 == A11Y_TAB
        }
    };
    // Reduce motion: a moved target under the latch is drawn AT the target with nothing running; without it, from where it was.
    use draw::motion::{Curve, Glides, BASE};
    let rm0 = A11Y_REDUCE.swap(true, Ordering::Relaxed);
    let snap = { let mut g = Glides::new(); let _ = g.to(1, 0, 1000, BASE, Curve::Standard); let v = g.to(1, 100, 1001, BASE, Curve::Standard); v == 100 && !g.running() };
    A11Y_REDUCE.store(false, Ordering::Relaxed);
    let eases = { let mut g = Glides::new(); let _ = g.to(1, 0, 1000, BASE, Curve::Standard); let v = g.to(1, 100, 1001, BASE, Curve::Standard); v < 100 && g.running() };
    A11Y_REDUCE.store(rm0, Ordering::Relaxed);
    let ic0 = A11Y_CONTRAST.swap(true, Ordering::Relaxed);
    let contrast = increase_contrast() && { A11Y_CONTRAST.store(false, Ordering::Relaxed); !increase_contrast() };
    A11Y_CONTRAST.store(ic0, Ordering::Relaxed);
    let q0 = A11Y_CURSOR_Q.swap(8, Ordering::Relaxed);
    let max = crate::ui::SCALE_MAX;
    let mut cursor = cursor_blocks(1) == 2 && cursor_blocks(2) == 4usize.min(max) && cursor_blocks(max) == max;
    A11Y_CURSOR_Q.store(A11Y_CURSOR_MAX_Q, Ordering::Relaxed);
    cursor &= cursor_blocks(1) == 4usize.min(max) && cursor_blocks(2) == max;
    A11Y_CURSOR_Q.store(A11Y_CURSOR_MIN_Q, Ordering::Relaxed);
    cursor &= cursor_blocks(1) == 1 && cursor_blocks(2) == 2 && a11y_cursor_q_of(2.0) == 8 && a11y_cursor_q_of(9.0) == 16 && a11y_cursor_q_of(0.1) == 4 && a11y_cursor_q_of(f64::NAN) == 4 && a11y_cursor_at(TRACK_X) == 4 && a11y_cursor_at(TRACK_X + TRACK_W) == 16;
    A11Y_CURSOR_Q.store(q0, Ordering::Relaxed);
    let text = A11Y_TEXT_PX[0] == font_core::ui::DEFAULT_CSS_PX && A11Y_TEXT_PX.windows(2).all(|w| w[0] < w[1]) && A11Y_TEXT_PX[3] >= 2 * A11Y_TEXT_PX[0]
        && A11Y_TEXT_PX.iter().all(|p| (FONT_MIN..=FONT_MAX).contains(p)) && a11y_text_step_of(16) == Some(1) && a11y_text_step_of(14).is_none()
        && A11Y_TEXT_NAMES.iter().all(|n| matches!(prefs_core::schema::lookup(crate::prefs::NS, key::A11Y_TEXT_SIZE).map(|k| k.kind), Some(prefs_core::schema::Kind::Enum(e)) if e.contains(n)));
    let ok = declared == keys.len() && pane && snap && eases && contrast && cursor && text;
    serial_println!(
        ":: A11Y: keys={} declared={} pane={} reduce_motion=snap({}) contrast={} cursor={}(x2) text={} -> {} ::",
        keys.len(), if declared == keys.len() { "ok" } else { "FAIL" }, if pane { "ok" } else { "FAIL" }, if snap && eases { "ok" } else if snap { "no-ease" } else { "FAIL" },
        if contrast { "ok" } else { "FAIL" }, if cursor { "ok" } else { "FAIL" }, if text { "ok" } else { "FAIL" }, if ok { "PASS" } else { "FAIL" }
    );
}

/// `tests a11yfilters` (A11YFILTERS, rmbp-ledger B705; BP-video-25): (1) each of the five `cvd` modes, and invert,
/// SET on the live latch, then three probe pixels copied by the CPU blitter's `CopyFilter` (the window present's op)
/// from an unfiltered RAM row into a RAM surface and READ BACK — each within 1 per channel of the reference computed
/// from Machado 2009's published decimals in f64 (`a11yfilter::reference`), and equal to the solid-colour path
/// (`a11yfilter::color`); the gray leg runs on an `Rgb` surface, the rest on `Bgr`; (2) the transform over a 1920x1200
/// RAM frame (deutan, the full matrix) timed — printed against the 1 ms bar, not gated on it; (3) the motion seam:
/// with Reduce motion latched, `motion::dur` answers 0 for each of the four role durations (and the role's own value
/// without it). Every latch is restored; nothing is written to the store.
/// `[a11y] filter=deutan pass_us=<n> motion=<full|reduced> frame=1920x1200 budget_us=1000 under=<0|1> why=fixture`
/// `:: A11YFILTERS: modes=5 px_ok=<n> pass_us=<n> motion_zero=<n> -> PASS ::`
pub fn selftest_a11yfilters() {
    use super::blitter::{self as bl, BlitJob, Op, Surface};
    use unaos_boot_info::{FrameBufferInfo, PixelFormat};
    let (m0, i0) = (af::mode(), af::invert());
    let probes = [theme::fixture::RGB_RED, theme::fixture::RGB_GREEN, theme::fixture::STEEL];
    let legs: [(usize, bool, PixelFormat); 6] = [
        (0, false, PixelFormat::Bgr), (1, false, PixelFormat::Bgr), (2, false, PixelFormat::Bgr),
        (3, false, PixelFormat::Bgr), (4, false, PixelFormat::Rgb), (0, true, PixelFormat::Bgr),
    ];
    let total = legs.len() * probes.len();
    let mut px_ok = 0usize;
    let mut modes = [false; af::MODES];
    for &(m, inv, fmt) in legs.iter() {
        let rgb = fmt == PixelFormat::Rgb;
        let mut src = [0u8; 12];
        let mut dst = [0u8; 16];
        for (k, &c) in probes.iter().enumerate() {
            let (r, g, b) = ((c >> 16) as u8, (c >> 8) as u8, c as u8);
            src[4 * k..4 * k + 4].copy_from_slice(&if rgb { [r, g, b, 0] } else { [b, g, r, 0] });
        }
        let mk = |base: usize, len: usize, w: usize| {
            let mut f = super::FrameBuffer::new();
            f.init(base, len, FrameBufferInfo { width: w, height: 1, stride: w, bytes_per_pixel: 4, pixel_format: fmt });
            f
        };
        let (sfb, dfb) = (mk(src.as_ptr() as usize, src.len(), 3), mk(dst.as_mut_ptr() as usize, dst.len(), 4));
        af::set(m, inv);
        let rect = [bl::Rect { sx: 0, sy: 0, dx: 1, dy: 0, w: 3, h: 1 }];
        let (res, _) = bl::run_on(&bl::CPU, &BlitJob { src: Surface::ram(sfb), dst: Surface::ram(dfb), rects: &rect, op: Op::CopyFilter });
        let mut leg_ok = true;
        for (k, &c) in probes.iter().enumerate() {
            let p = &dst[4 * (k + 1)..4 * (k + 2)];
            let got = if rgb { (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32 } else { (p[2] as u32) << 16 | (p[1] as u32) << 8 | p[0] as u32 };
            let want = af::reference(c, m, inv);
            let close = (0..3).all(|sh| ((got >> (8 * sh)) & 255).abs_diff((want >> (8 * sh)) & 255) <= 1);
            if res.is_ok() && close && af::color(c) == got {
                px_ok += 1;
            } else {
                leg_ok = false;
                serial_println!("[a11y] px mode={} invert={} in={:06x} got={:06x} want={:06x} blit_ok={}", af::NAMES[m], inv as u8, c, got, want, res.is_ok() as u8);
            }
        }
        if leg_ok && !inv { modes[m] = true; }
    }
    af::set(m0, i0);
    // (2) the full-frame transform.
    const FW: usize = 1920;
    const FH: usize = 1200;
    const BUDGET_US: u64 = 1000;
    let mut frame: Vec<u8> = Vec::new();
    let pass_us = if frame.try_reserve_exact(FW * FH * 4).is_ok() {
        frame.resize(FW * FH * 4, 0);
        for (i, b) in frame.iter_mut().enumerate() { *b = (i ^ (i >> 11)) as u8; }
        let t0 = crate::arch::now_cycles();
        af::transform(PixelFormat::Bgr, 2, false, &mut frame);
        Some(bl::cycles_to_us(crate::arch::now_cycles().saturating_sub(t0)))
    } else {
        None
    };
    drop(frame);
    let _ = af::take_stats(); // the fixture's own row transforms are not the glass's cost
    // (3) the motion seam, on the live latch.
    use draw::motion::{dur, BASE, ENTER, EXIT, FAST};
    let rm0 = A11Y_REDUCE.swap(true, Ordering::Relaxed);
    let motion_zero = [FAST, BASE, ENTER, EXIT].iter().filter(|&&d| dur(d) == 0).count();
    A11Y_REDUCE.store(false, Ordering::Relaxed);
    let full = [FAST, BASE, ENTER, EXIT].iter().all(|&d| dur(d) == d && d > 0);
    A11Y_REDUCE.store(rm0, Ordering::Relaxed);
    let nmodes = modes.iter().filter(|&&x| x).count();
    let us = pass_us.unwrap_or(0);
    serial_println!(
        "[a11y] filter=deutan pass_us={} motion={} frame={}x{} budget_us={} under={} px_ok={}/{} why=fixture",
        us, if draw::motion::reduced() { "reduced" } else { "full" }, FW, FH, BUDGET_US, (pass_us.is_some() && us <= BUDGET_US) as u8, px_ok, total
    );
    let ok = nmodes == af::MODES && px_ok == total && pass_us.is_some() && motion_zero == 4 && full;
    serial_println!(
        ":: A11YFILTERS: modes={} px_ok={} pass_us={} motion_zero={} -> {} ::",
        nmodes, px_ok, if pass_us.is_some() { alloc::format!("{}", us) } else { String::from("skip(heap)") }, motion_zero, if ok { "PASS" } else { "FAIL" }
    );
}

// ── REASK (rmbp-ledger B681, R130 law 2) — the Privacy tab ────────────────────────────────────────────────────────
// Row 0 is the tab's one MAP row: `Remember my answers` (`system.privacy.remember`, a switch; off = a new answer is not
// kept) with the count of answers kept; rows 1..=PRIV_ROWS one decision each (`<domain>: <subject's leaf>`, the answer and its
// date, a Revoke at the right); the row after them `+<n> more`. Revoke is LATCHED in the click router and run by
// [`privacy_service`] on the service pass (`prefs::revoke` saves a domain file — never in the router).

/// The Privacy tab's index on the strip: Date & Time (TZLOCALE B701), Keyboard (KEYMAPS B704) and Storage (SMARTLINE B708) follow it.
pub const PRIV_TAB: usize = 10;
/// SMARTLINE (B708): the Storage tab; AUTOLOCK (B712): Security after it, the last.
pub const STORAGE_TAB: usize = 13;
const _: () = assert!(HW_TAB == TABS - 1 && SEC_TAB + 1 == HW_TAB && STORAGE_TAB + 1 == SEC_TAB && KB_TAB + 1 == STORAGE_TAB && DT_TAB + 1 == KB_TAB && PRIV_TAB + 1 == DT_TAB && A11Y_TAB + 1 == PRIV_TAB && NP_TAB + 1 == A11Y_TAB); // the fold: 11, 12, 13, 14, 15 (HWTAB B737)
/// Decision rows shown.
const PRIV_ROWS: usize = 8;
/// The Revoke button's x (right-aligned in the row).
const PRIV_REVOKE_X: usize = CTRL_R - BTN_W;
/// A revoke the router latched (the decision's key), run by [`privacy_service`].
static PRIV_REVOKE: crate::sync::Mutex<Option<String>> = crate::sync::Mutex::new(None);
/// The Remember switch: the value this window last chose (`u32::MAX` = read the store) and its owed store write.
static PRIV_REMEMBER_SHOWN: AtomicU32 = AtomicU32::new(u32::MAX);
static PRIV_REMEMBER_OWED: AtomicU32 = AtomicU32::new(u32::MAX);

/// Are new answers kept? This window's last choice, else the store's (default on).
fn privacy_remember() -> bool {
    match PRIV_REMEMBER_SHOWN.load(Ordering::Relaxed) {
        u32::MAX => crate::prefs::flag(crate::prefs::key::PRIVACY_REMEMBER).unwrap_or(true),
        v => v != 0,
    }
}

fn paint_privacy(st: &mut State) {
    let d = crate::prefs::decisions();
    let on = privacy_remember();
    groups(st, &[(0, 0), (1, d.len().clamp(1, PRIV_ROWS))]);
    txt(st, LABEL_X, 0, "Remember my answers");
    tog(st, 0, TOG_X, on, false);
    txt2(st, TRACK_X, 0, &alloc::format!("{} kept; revoke one to be asked again", d.len()));
    if d.is_empty() {
        txt2(st, LABEL_X, 1, "None yet: an ask you answer is kept here.");
    }
    for (k, x) in d.iter().take(PRIV_ROWS).enumerate() {
        let leaf = x.who.trim_end_matches('/').rsplit('/').next().unwrap_or(&x.who);
        let leaf: String = leaf.chars().take(28).collect(); let (leaf, dst) = if x.domain == crate::prefs::decide::NET { let (p, d) = crate::fs::apptrust::net_row(&x.who); (p.chars().take(28).collect::<String>(), d) } else { (leaf, None) }; // GRANTNAMES (B742): a net row names the program (`<FILE> on <volume>` off the system's storage) and where it went; in-place fold
        txt(st, LABEL_X, 1 + k, &alloc::format!("{}: {}", x.domain, leaf));
        let when = if x.at == 0 {
            String::from("date unknown")
        } else {
            let mut b = [0u8; 10]; // CIVILREST (B738): the answer's date in Principia's locale
            let n = crate::clock::locale::fmt_date(crate::clock::locale::Painter::Privacy, x.at, &mut b);
            String::from(core::str::from_utf8(&b[..n]).unwrap_or(""))
        };
        match &dst { Some(d) => txt2(st, TRACK_X, 1 + k, &alloc::format!("{} · {}", d, x.answer)), None => txt2(st, TRACK_X, 1 + k, &alloc::format!("{} ({})", x.answer, when)) } // GRANTNAMES (B742): `<host> (<ip>) · <answer>` for a net row this boot recorded (in-place edit)
        btn(st, 1 + k, PRIV_REVOKE_X, "Revoke");
    }
    if d.len() > PRIV_ROWS {
        txt2(st, LABEL_X, 1 + PRIV_ROWS, &alloc::format!("+{} more", d.len() - PRIV_ROWS));
    }
}

/// A press on the Privacy tab: a row's Revoke latches that decision's key (no store write in the router).
fn press_privacy(row: usize, cx: usize) {
    if row == 0 {
        let v = !privacy_remember();
        PRIV_REMEMBER_SHOWN.store(v as u32, Ordering::Relaxed);
        PRIV_REMEMBER_OWED.store(v as u32, Ordering::Release);
        say("privacy.remember", if v { "true" } else { "false" }, true);
        repaint();
        return;
    }
    if !(1..=PRIV_ROWS).contains(&row) || cx < PRIV_REVOKE_X || cx >= PRIV_REVOKE_X + BTN_W {
        return;
    }
    let d = crate::prefs::decisions();
    let Some(x) = d.get(row - 1) else { return };
    if let Some(mut g) = PRIV_REVOKE.try_lock() {
        *g = Some(x.key.clone());
        serial_println!("[settings] privacy revoke key={} latched", x.key);
    }
}

/// The service pass: run a latched Revoke, repaint the open Privacy pane.
fn privacy_service() {
    let v = PRIV_REMEMBER_OWED.swap(u32::MAX, Ordering::AcqRel); if crate::fs::apptrust::net_rows_settle() && is_open() && cur_tab() == PRIV_TAB { repaint(); } // GRANTNAMES (B742): the net rows' volumes placed off the syscall path. LINE-NEUTRAL append, code first
    if v != u32::MAX {
        crate::prefs::set_sys(crate::prefs::key::PRIVACY_REMEMBER, crate::prefs::PrefValue::Bool(v != 0));
    }
    let Some(k) = PRIV_REVOKE.try_lock().and_then(|mut g| g.take()) else { return }; #[cfg(feature = "uvc")] crate::drivers::xhci::uvc::revoked(&k); #[cfg(feature = "uac")] crate::fs::apptrust::mic_revoked(&k); crate::fs::apptrust::net_revoked(&k); /* GRANTNAMES (B742): a revoked net decision drops the app's run rows too (Allow Once), so the running task asks again */ // CAMDOT (B702): the camera's decision stops its stream BEFORE the store write (the viewer's next 8 ms pass), so the revoke is inside one frame and the bar's dot goes out with it. LINE-NEUTRAL append, code first.
    let ok = crate::prefs::revoke(&k);
    serial_println!("[settings] privacy revoke key={} ok={}", k, ok as u8);
    if is_open() && cur_tab() == PRIV_TAB {
        repaint();
    }
}

// ── TZLOCALE (rmbp-ledger B701, BP-bar-17) — the Date & Time tab ──────────────────────────────────────────────────────
// One row per `system.locale.*` key (BP-bar-17): row 0 Time zone (- / + walk `clock::tz::ZONES`), row 1 Date and time
// format (the three date orders as segments, the 24-hour switch at the right), row 2 First day of week (segments); row
// 3 the stamp the painters print now, with its offset. A press applies the choice live (`clock::locale::choose_*`) and
// the store write is owed to `clock::locale::service` on the service pass (never in the click router).

/// The Date & Time tab's index on the strip: the last tab.
pub const DT_TAB: usize = 11;
const DT_ORDERS: [&[u8]; 3] = [b"Y-M-D", b"D/M/Y", b"M/D/Y"];
const DT_FIRST: [&[u8]; 3] = [b"Monday", b"Sunday", b"Saturday"];

fn paint_datetime(st: &mut State) {
    use crate::clock::locale;
    groups(st, &[(0, 0), (1, 2), (3, 3)]);
    let l = locale::current();
    txt(st, LABEL_X, 0, "Time zone");
    btn(st, 0, TRACK_X, "-");
    btn(st, 0, TRACK_X + BTN_W + 10, "+");
    txt2(st, TRACK_X + 2 * BTN_W + 24, 0, l.zone().name);
    txt(st, LABEL_X, 1, "Date and time format");
    seg(st, 1, &DT_ORDERS, l.order() as usize, false);
    txt2(st, VAL_X, 1, "24h");
    tog(st, 1, TOG_X, l.h24(), false);
    txt(st, LABEL_X, 2, "First day of week");
    seg(st, 2, &DT_FIRST, l.first_day as usize, false);
    txt(st, LABEL_X, 3, "Now");
    let now = match crate::clock::try_unix_now() {
        Some(u) => {
            let (_, off, dst) = l.local(u);
            let o = locale::off_text(off);
            alloc::format!("{}  UTC{}{}", locale::fmt_stamp_in(&l, u), core::str::from_utf8(&o).unwrap_or(""), if dst { " daylight" } else { "" })
        }
        None => String::from("The clock is not set"),
    };
    txt2(st, TRACK_X, 3, &now);
}

/// A press on the Date & Time tab: - / + walk the zones, a segment picks the order or the first day, the switch flips
/// the hour cycle. Each applies at once; the store write is `clock::locale::service`'s.
fn press_datetime(row: usize, cx: usize) {
    use crate::clock::{locale, tz};
    let l = locale::current();
    let n = tz::ZONES.len();
    match row {
        0 if cx >= TRACK_X && cx < TRACK_X + BTN_W => locale::choose_tz((l.tz as usize + n - 1) % n),
        0 if cx >= TRACK_X + BTN_W + 10 && cx < TRACK_X + 2 * BTN_W + 10 => locale::choose_tz((l.tz as usize + 1) % n),
        1 if cx >= TRACK_X && cx < TRACK_X + TRACK_W => {
            let o = ((cx - TRACK_X) / (TRACK_W / DT_ORDERS.len())).min(DT_ORDERS.len() - 1);
            locale::choose_region(o * 2 + (!l.h24()) as usize);
        }
        1 if cx >= TOG_X && cx < TOG_X + draw::DIMS.toggle_w => locale::choose_region(l.order() as usize * 2 + l.h24() as usize),
        2 if cx >= TRACK_X && cx < TRACK_X + TRACK_W => locale::choose_first(((cx - TRACK_X) / (TRACK_W / DT_FIRST.len())).min(DT_FIRST.len() - 1)),
        _ => return,
    }
    let l = locale::current();
    say("locale", &alloc::format!("{}/{}/{}", l.zone().name, l.region_name(), l.first_day_name()), true);
    repaint();
}

// ── SETTINGSMAP (rmbp-ledger B673, BP-bar-17, R118) — one row per preference, and the search that reaches it ─────────
// "One Settings row per preference" (BAR row 17): [`MAP`] is the ONE table of Settings rows. A row names the Principia
// keys it edits (`system.<key>`, every one in `prefs::key::ALL`), the tab and row it is painted on, the label it paints
// and the keyboard control that edits it (`NO_CTRL` = a mouse row). Every `prefs::key::ALL` key is on exactly one row;
// `system.settings.tab` is on the tab COLUMN (its control is the column). The search field reads the table
// ([`col_list`], [`search_best`]), so does the launcher ([`row_of_key`]); `tests settingsmap` holds both directions and
// that every row's label is reached by at most two typed keys and Return.

/// The `tab` of a row that lives on the tab column itself (`system.settings.tab`: the column is its control).
pub const COLUMN: usize = TABS;
/// A row with no keyboard control (a mouse row, a list, a link): Return selects its tab and rings the row.
const NO_CTRL: usize = AT_FIRST;

/// One Settings row: the keys it edits, where it is painted, its label as painted, its keyboard control.
#[derive(Clone, Copy)]
pub struct MapRow {
    pub keys: &'static [&'static str],
    pub tab: usize,
    pub row: usize,
    pub label: &'static str,
    pub ctrl: usize,
}

const fn mr(keys: &'static [&'static str], tab: usize, row: usize, label: &'static str, ctrl: usize) -> MapRow {
    MapRow { keys, tab, row, label, ctrl }
}

/// Every Settings row, in column order then row order. Labels are the strings the panes paint.
pub const MAP: [MapRow; 45] = {
    use crate::prefs::key::*;
    [
        // General (0)
        mr(&[VOLUME], 0, 0, "Volume", 1),
        mr(&[MUTE, AUDIO_OUTPUT], 0, 1, "Mute", 2), // UAC1 (B706): the output picker shares the Mute row
        mr(&[POINTER], 0, 2, "Pointer", NO_CTRL),
        mr(&[WALLPAPER], 0, 3, "Wallpaper", 5),
        mr(&[DOCK_POSITION], 0, 6, "Dock position", NO_CTRL),
        mr(&[DOCK_AUTOHIDE], 0, 7, "Automatically hide the dock", NO_CTRL),
        mr(&[DOCK_PINS], 0, 8, "Dock pins", NO_CTRL),
        // Display (2)
        mr(&[BRIGHTNESS], 2, 0, "Brightness", 0),
        mr(&[IDLE_MIN], 2, 1, "Blank screen", 3),
        mr(&[DISPLAY_MODE], 2, 2, "Resolution", NO_CTRL),
        mr(&[LOWBAT_PCT], 2, LOWBAT_ROW, "Low battery shutdown", NO_CTRL),
        mr(&[FONT], 2, 4, "Font", 9),
        mr(&[FONT_SIZE], 2, 5, "Font size", 10),
        // Login Items (4)
        mr(&[LOGIN_ITEMS], 4, 0, "Open at login", NO_CTRL),
        // Appearance (5)
        mr(&[APPEARANCE_MODE], 5, 0, "Appearance", NO_CTRL),
        mr(&[APPEARANCE_ACCENT], 5, 2, "Accent colour", NO_CTRL),
        mr(&[APPEARANCE_HIGHLIGHT], 5, 3, "Highlight colour", NO_CTRL),
        mr(&[LOOK], 5, 7, "Look", NO_CTRL),
        // Trackpad
        mr(&[TP_SPEED], TP_TAB, 0, "Tracking speed", NO_CTRL),
        mr(&[TP_TAP], TP_TAB, 1, TP_ROW_NAMES[0], NO_CTRL),
        mr(&[TP_NATURAL], TP_TAB, 2, TP_ROW_NAMES[1], NO_CTRL),
        mr(&[TP_SECONDARY], TP_TAB, 3, TP_ROW_NAMES[2], NO_CTRL),
        mr(&[TP_THREE_DRAG], TP_TAB, 4, TP_ROW_NAMES[3], NO_CTRL),
        // File Types
        mr(&[QUARRY_FAVORITES], FT_TAB, FT_FAV_ROW, "Sidebar favorites", NO_CTRL),
        // Notifications
        mr(&[NOTIFY_DND], NP_TAB, 0, "Do Not Disturb", NO_CTRL),
        mr(&[NOTIFY_DND_FROM, NOTIFY_DND_UNTIL], NP_TAB, 1, "Scheduled", NO_CTRL),
        // Accessibility
        mr(&[A11Y_REDUCE_MOTION], A11Y_TAB, 0, "Reduce motion", NO_CTRL),
        mr(&[A11Y_INCREASE_CONTRAST], A11Y_TAB, 1, "Increase contrast", NO_CTRL),
        mr(&[A11Y_CURSOR_SIZE], A11Y_TAB, 2, "Cursor size", NO_CTRL),
        mr(&[A11Y_TEXT_SIZE], A11Y_TAB, 3, "Text size", NO_CTRL),
        mr(&[A11Y_CVD], A11Y_TAB, 4, "Colour filter", NO_CTRL),
        mr(&[A11Y_INVERT], A11Y_TAB, 5, "Invert colours", NO_CTRL),
        mr(&[A11Y_ZOOM], A11Y_TAB, 6, "Zoom", NO_CTRL), // ZOOM (B713)
        mr(&[A11Y_ZOOM_FOLLOW], A11Y_TAB, 7, "Zoom follows", NO_CTRL),
        mr(&[A11Y_SHOW_PERF], A11Y_TAB, 8, "Show performance", NO_CTRL), // PERFHUD (B710)
        // Privacy (REASK B681): the switch row; the decisions below it are dynamic (`prefs::decisions`), each with a Revoke
        mr(&[PRIVACY_REMEMBER], PRIV_TAB, 0, "Remember my answers", NO_CTRL),
        // Date & Time (TZLOCALE B701): the one locale authority's three keys
        mr(&[LOCALE_TZ], DT_TAB, 0, "Time zone", NO_CTRL),
        mr(&[LOCALE_REGION], DT_TAB, 1, "Date and time format", NO_CTRL),
        mr(&[LOCALE_FIRST_DAY], DT_TAB, 2, "First day of week", NO_CTRL),
        // Keyboard (KEYMAPS B704): the layout, the Compose tap
        mr(&[KBD_LAYOUT], KB_TAB, 0, "Layout", NO_CTRL),
        mr(&[KBD_COMPOSE], KB_TAB, 1, "Compose key", NO_CTRL),
        mr(&[LOCK_AFTER_IDLE_MIN], SEC_TAB, 0, SEC_ROW_NAMES[0], NO_CTRL), mr(&[LOCK_ON_LID], SEC_TAB, 1, SEC_ROW_NAMES[1], NO_CTRL), mr(&[LOCK_ON_SLEEP], SEC_TAB, 2, SEC_ROW_NAMES[2], NO_CTRL), // Security (AUTOLOCK B712): one row per key. LINE-NEUTRAL fold
        // The tab column
        mr(&[SETTINGS_TAB], COLUMN, 0, "Settings tabs", NO_CTRL),
    ]
};

/// How well `text` holds the query `q`, ASCII case-insensitive: 0 = it starts with `q`, 1 = a word in it does (after
/// a byte that is not a letter or digit), 2 = `q` is inside it; `None` = not at all (and for the empty query). Pure.
fn hold_rank(text: &str, q: &str) -> Option<u8> {
    let (t, q) = (text.as_bytes(), q.as_bytes());
    if q.is_empty() || q.len() > t.len() { return None; }
    let at = |i: usize| t[i..i + q.len()].eq_ignore_ascii_case(q);
    let last = t.len() - q.len();
    if at(0) { return Some(0); }
    if (1..=last).any(|i| !t[i - 1].is_ascii_alphanumeric() && at(i)) { return Some(1); }
    if (1..=last).any(at) { return Some(2); }
    None
}

/// What the search's Return opens: a row of [`MAP`], or a tab whose name the text matched.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit { Row(usize), Tab(usize) }

/// The search's best hit for `q`: the lowest [`hold_rank`]; at the same rank a row before a tab, then table order. Pure.
pub fn search_best(q: &str) -> Option<Hit> {
    let rows = MAP.iter().enumerate().filter_map(|(i, r)| hold_rank(r.label, q).map(|k| (k, 0u8, i)));
    let tabs = TAB_NAMES.iter().enumerate().filter_map(|(i, n)| hold_rank(n, q).map(|k| (k, 1u8, i)));
    rows.chain(tabs).min().map(|(_, kind, i)| if kind == 0 { Hit::Row(i) } else { Hit::Tab(i) })
}

/// Open a hit: a row's tab, its control selected when it has a keyboard one (else focus stays on the column) and the
/// row ringed; the column row focuses the column; a tab opens as the column's pick did. No lock held by the caller.
fn open_hit(h: Hit, typed: usize) {
    let i = match h {
        Hit::Tab(t) => { if t != cur_tab() { switch_tab(t); } else { repaint(); } return; }
        Hit::Row(i) => i,
    };
    let r = MAP[i];
    if r.tab < TABS && r.tab != cur_tab() { switch_tab(r.tab); }
    if let Some(st) = STATE.lock().as_mut() {
        let keyed = r.tab < TABS && tab_ctrls(r.tab).contains(&r.ctrl);
        st.strip = !keyed;
        if keyed { st.sel = r.ctrl; }
        st.found = (r.tab < TABS).then_some((r.tab as u8, r.row as u8));
    }
    let tn = if r.tab < TABS { TAB_NAMES[r.tab] } else { "column" };
    serial_println!("[settings] search hit row={} tab={} key=system.{} typed={}", r.label, tn, r.keys.first().copied().unwrap_or(""), typed);
    repaint();
}

/// The accent ring around row `r` (the row the search opened).
fn found_ring(st: &mut State, r: usize) {
    let (x, y) = (BOX_X + GAP, row_top(r) + GAP);
    let rr = Rect::new(e(x), e(y), ew(x, BOX_W - 2 * GAP), ew(y, ROW_H - 2 * GAP));
    draw::stroke_rrect(&mut sf(st), rr, dp(draw::DIMS.row_radius), dp(2).max(1), Ink::argb(theme::accent()));
}

/// The Display tab's Low battery shutdown row (`system.power.lowbat_shutdown_pct`, 0 = off) and its step.
const LOWBAT_ROW: usize = 3;
const LOWBAT_STEP: i64 = 5;
/// The value this window last set (`u32::MAX` = read the store), and a store write owed to the service pass.
static LOWBAT_SHOWN: AtomicU32 = AtomicU32::new(u32::MAX);
static LOWBAT_OWED: AtomicU32 = AtomicU32::new(u32::MAX);

/// The shutdown percent the row shows: this window's last choice, else the one in force (the store, else the build's).
fn lowbat_now() -> i64 {
    let s = LOWBAT_SHOWN.load(Ordering::Relaxed);
    if s != u32::MAX { return s as i64; }
    #[cfg(feature = "lowbat_shutdown")]
    { super::powerui::lowbat_shutdown_pct() as i64 }
    #[cfg(not(feature = "lowbat_shutdown"))]
    { crate::prefs::peek_int(crate::prefs::key::LOWBAT_PCT, 0, 100).unwrap_or(0) }
}

/// - / + on the row: one step, held to the schema's 0..=100; the store write is latched for [`lowbat_service`].
fn lowbat_step(up: bool) {
    let v = (lowbat_now() + if up { LOWBAT_STEP } else { -LOWBAT_STEP }).clamp(0, 100) as u32;
    LOWBAT_SHOWN.store(v, Ordering::Relaxed);
    LOWBAT_OWED.store(v, Ordering::Release);
    say("lowbat_shutdown_pct", &alloc::format!("{}", v), true);
    repaint();
}

/// Drain the row's owed store write (the service pass; never a bus write in the click router).
fn lowbat_service() {
    let v = LOWBAT_OWED.swap(u32::MAX, Ordering::AcqRel);
    if v != u32::MAX { crate::prefs_client::sys_set(crate::prefs::key::LOWBAT_PCT, crate::prefs::PrefValue::Int(v as i64)); }
}

/// The shortest typed text (one key, else two) whose Return opens row `i`: built from the label's own bytes, letters and
/// digits first. `None` = no text of two keys or fewer reaches it. Pure.
fn reach(i: usize) -> Option<String> {
    let mut ch: Vec<u8> = MAP[i].label.bytes().map(|b| b.to_ascii_lowercase()).collect();
    ch.sort_by_key(|&b| (!b.is_ascii_alphanumeric(), b));
    ch.dedup();
    let hits = |q: &String| search_best(q) == Some(Hit::Row(i));
    for &a in &ch {
        let q = String::from(a as char);
        if hits(&q) { return Some(q); }
    }
    for &a in &ch {
        for &b in &ch {
            let mut q = String::from(a as char);
            q.push(b as char);
            if hits(&q) { return Some(q); }
        }
    }
    None
}

/// `tests settingsmap`: every `prefs::key::ALL` key on exactly one row (`unrowed` counts the keys on none or on more
/// than one); every row's keys in ALL and declared by the schema, its place on the window, no two rows on one place
/// (`orphans`); the four keys ALL spells beside their owners' constants equal to them; every row's label reached by
/// at most two typed keys and Return ([`reach`]); then the hardest row driven LIVE through the search field
/// (`search_key`: its keys, Return) and the window read back (its tab, the ring, the selected control).
pub fn selftest_map() {
    use crate::prefs::key;
    let keys = key::ALL.len();
    let unrowed = key::ALL.iter().filter(|&&k| MAP.iter().filter(|r| r.keys.contains(&k)).count() != 1).count();
    let placed = |r: &MapRow| if r.tab < TABS { r.row < ROWS } else { r.tab == COLUMN };
    let orphans = MAP.iter().enumerate().filter(|&(i, r)| {
        r.keys.is_empty()
            || r.keys.iter().any(|k| !key::ALL.contains(k) || prefs_core::schema::lookup(crate::prefs::NS, k).is_none())
            || !placed(r)
            || MAP[..i].iter().any(|o| o.tab == r.tab && o.row == r.row)
    }).count();
    let same = key::LOOK == prefs_core::appearance::LOOK_KEY && key::NOTIFY_DND == super::notify::KEY_DND
        && key::NOTIFY_DND_FROM == super::notifypane::KEY_DND_FROM && key::NOTIFY_DND_UNTIL == super::notifypane::KEY_DND_UNTIL;
    // The search leg: the hardest row (most keys; the first such in table order), or the first row no text reaches.
    let mut hard: Option<(usize, String)> = None;
    let mut unreached: Option<usize> = None;
    for i in 0..MAP.len() {
        match reach(i) {
            Some(q) => { if hard.as_ref().map_or(true, |(_, h)| q.len() > h.len()) { hard = Some((i, q)); } }
            None => { if unreached.is_none() { unreached = Some(i); } }
        }
    }
    let search = match (unreached, &hard) {
        (Some(i), _) => alloc::format!("bad({}:none)", MAP[i].label),
        (None, Some((i, q))) => alloc::format!("ok({}:{})", MAP[*i].label, q.len()),
        (None, None) => String::from("bad(empty)"),
    };
    // The live leg: type the hardest row's text into the open window's search field and press Return.
    let live = match (&hard, unreached) {
        (Some((i, q)), None) => {
            let was_open = is_open();
            if was_open || open().is_ok() {
                let tab0 = cur_tab();
                if let Some(x) = STATE.lock().as_mut() { x.query.clear(); x.search = true; x.found = None; }
                for b in q.bytes() { search_key(crate::pal::Event::Key(b)); }
                search_key(crate::pal::Event::Key(0x0D));
                let r = MAP[*i];
                let (found, strip, sel) = STATE.lock().as_ref().map_or((None, true, usize::MAX), |x| (x.found, x.strip, x.sel));
                let ok = if r.tab < TABS {
                    cur_tab() == r.tab && found == Some((r.tab as u8, r.row as u8))
                        && (if tab_ctrls(r.tab).contains(&r.ctrl) { !strip && sel == r.ctrl } else { strip })
                } else {
                    strip && found.is_none() && cur_tab() == tab0
                };
                if let Some(x) = STATE.lock().as_mut() { x.query.clear(); x.search = false; x.found = None; }
                if cur_tab() != tab0 { switch_tab(tab0); }
                if !was_open { close(); }
                if ok { "ok" } else { "bad" }
            } else {
                "skip(no-window)"
            }
        }
        _ => "bad",
    };
    let ok = unrowed == 0 && orphans == 0 && same && search.starts_with("ok(") && live != "bad";
    serial_println!(
        ":: SETTINGSMAP: keys={} rows={} orphans={} unrowed={} search={} -> {} :: live={} owners_same={}",
        keys, MAP.len(), orphans, unrowed, search, if ok { "PASS" } else { "FAIL" }, live, same as u8
    );
}

// ── A11YTREE (rmbp-ledger B675; BP-bar-22: "Tab through Settings shows a ≥ 2 px ring at every stop") ──────────────
// Every kit control draws DESIGN-LANGUAGE §2.7's ring when it holds the keyboard (`draw::focus_ring`, the
// `focus_ring` token, `DIMS.focus_ring` logical px device-scaled), and the column's pill does (`st.strip && st.kbd`).
// The one stop that showed none was a Users row: Tab walks `u.sel` down the list and the row got only its wash.
// [`row_ring`] is that ring, the kit's arithmetic on the wash's rect. [`ring_probe`] is `tests a11ytree`'s ring leg.

/// The kit's focus ring outside a Users row's wash (row `r`): width `DIMS.focus_ring` at the panel's scale, the radius
/// the wash's plus that width — what `draw::focus_ring` draws around a control.
fn row_ring(st: &mut State, r: usize) {
    let (x, y) = (BOX_X + GAP, row_top(r) + GAP);
    let rr = Rect::new(e(x), e(y), ew(x, BOX_W - 2 * GAP), ew(y, ROW_H - 2 * GAP));
    let w = dp(draw::DIMS.focus_ring);
    draw::stroke_rrect(&mut sf(st), rr.inset(-w), dp(draw::DIMS.row_radius) + w, w, Ink::argb(theme::focus_ring()));
}

/// `tests a11ytree`'s ring leg. Every stop Tab reaches — on each tab the column (stop 0), then the pane's keyboard
/// controls (`tab_ctrls`) or, on Users, each listed row — is painted OFFSCREEN holding the keyboard (`kbd`), and the
/// same tab is painted at rest (focus on the column, no key pressed). The ring is read from the difference: in the
/// first changed run of its top row, at the run's middle, the changed pixels downward that are the ring's colour (one
/// anti-aliased pixel skipped at the top). Returns `(stops, rung, ring_px, first stop without a ring as (tab, stop))`,
/// `ring_px` the thinnest ring seen, in device px. One reused pair of surfaces; a fresh `State` per paint, so no glide
/// carries between paints.
#[cfg(feature = "witness")]
pub fn ring_probe() -> (usize, usize, usize, Option<(usize, usize)>) {
    let (pw, ph) = (super::metrics::size(WIN_W), super::metrics::size(WIN_H));
    let (mut a, mut b): (Vec<u32>, Vec<u32>) = (Vec::new(), Vec::new());
    if a.try_reserve_exact(pw * ph).is_err() || b.try_reserve_exact(pw * ph).is_err() {
        return (0, 0, 0, Some((usize::MAX, 0)));
    }
    a.resize(pw * ph, 0);
    b.resize(pw * ph, 0);
    let v0 = CUR.lock().clone();
    let users = user_list().len().min(8);
    let paint_one = |buf: Vec<u32>, tab: usize, stop: Option<usize>| -> Vec<u32> {
        let mut st = State::new(WIN_W, WIN_H, buf, 1, true, 0);
        if let Some(k) = stop {
            st.kbd = true;
            if k > 0 {
                st.strip = false;
                if tab == 1 { st.u.sel = k - 1; } else { st.sel = tab_ctrls(tab)[k - 1]; }
            }
        }
        let mut v = v0.clone();
        v.tab = tab as u8;
        paint(&mut st, &v);
        core::mem::take(&mut st.surf)
    };
    let ring = theme::focus_ring() & 0x00FF_FFFF;
    let near = |p: u32| (0..3).all(|k| ((p >> (8 * k)) & 0xFF).abs_diff((ring >> (8 * k)) & 0xFF) <= 3);
    let (mut stops, mut rung, mut thin, mut fail) = (0usize, 0usize, usize::MAX, None);
    for tab in 0..TABS {
        b = paint_one(b, tab, None);
        let n = 1 + if tab == 1 { users } else { tab_ctrls(tab).len() };
        for k in 0..n {
            a = paint_one(a, tab, Some(k));
            stops += 1;
            let ch = |i: usize| (a[i] & 0x00FF_FFFF) != (b[i] & 0x00FF_FFFF);
            let px = match (0..pw * ph).find(|&i| ch(i)) {
                None => 0,
                Some(i0) => {
                    let (y0, x0) = (i0 / pw, i0 % pw);
                    let mut x1 = x0;
                    while x1 + 1 < pw && ch(y0 * pw + x1 + 1) { x1 += 1; }
                    let xm = (x0 + x1) / 2;
                    let mut y = y0;
                    while y < ph && y < y0 + 2 && ch(y * pw + xm) && !near(a[y * pw + xm]) { y += 1; }
                    let mut run = 0;
                    while y < ph && ch(y * pw + xm) && near(a[y * pw + xm]) { run += 1; y += 1; }
                    run
                }
            };
            if px >= 2 { rung += 1; thin = thin.min(px); } else if fail.is_none() { fail = Some((tab, k)); }
        }
    }
    (stops, rung, if thin == usize::MAX { 0 } else { thin }, fail)
}

// ── KEYMAPS (rmbp-ledger B704) — the Keyboard tab ─────────────────────────────────────────────────────────────────────
// Row 0 `Layout` (`system.keyboard.layout`, a segmented control over `keymap::layouts::LAYOUTS`), row 1 `Compose key`
// (`system.keyboard.compose`, a switch: a Right Alt tap composes). Read over the bus at login and on a PrefChanged naming
// `keyboard.*`; applied to the live layout the decoders' seam (`keymap::layout_report`) reads; a press latches the store
// write for [`kbd_service`] (never a bus write in the click router). Wire: `[kbd] layout=<name> dead=<n> compose=<0|1> via=`.

/// The Keyboard tab's index on the strip (Storage and Security follow it since the fold).
pub const KB_TAB: usize = 12;
const _: () = assert!(KB_TAB + 1 == STORAGE_TAB);
/// Store writes owed: bit 0 the layout, bit 1 the Compose switch.
static KB_PERSIST: AtomicU32 = AtomicU32::new(0);
/// A PrefChanged named a `keyboard.*` key: the service pass re-reads both.
static KB_DIRTY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Read the two keys over the bus (the schema's default where unset: `us`, off) and make them live. `via` = `login`
/// (always announced) or `prefchanged` (announced when something moved).
fn kbd_load(via: &str) {
    use crate::video::keymap::layouts as kl;
    use crate::prefs::key; use crate::prefs_client::{sys_flag, sys_text};
    let i = sys_text(key::KBD_LAYOUT).and_then(|s| kl::index_of(s.as_str())).unwrap_or(0);
    let c = sys_flag(key::KBD_COMPOSE).unwrap_or(false);
    let i0 = kl::index_of(kl::active().name).unwrap_or(0);
    if via == "login" || i != i0 || c != kl::compose_on() {
        kl::set(i, c, via);
    }
}

/// Row 0: layout `val`; row 1: Compose on when `val != 0`. Live at once, printed, the store write latched. Repaints.
fn kbd_set(row: usize, val: usize) {
    use crate::video::keymap::layouts as kl;
    let i = kl::index_of(kl::active().name).unwrap_or(0);
    match row {
        0 => { let v = val.min(kl::LAYOUTS.len() - 1); kl::set(v, kl::compose_on(), "settings"); say("keyboard.layout", kl::NAMES[v], true); }
        1 => { kl::set(i, val != 0, "settings"); say("keyboard.compose", if val != 0 { "true" } else { "false" }, true); }
        _ => return,
    }
    KB_PERSIST.fetch_or(1 << row, Ordering::AcqRel);
    repaint();
}

/// The service pass: a PrefChanged's re-read, then the owed store writes.
fn kbd_service() {
    use crate::video::keymap::layouts as kl;
    if KB_DIRTY.swap(false, Ordering::AcqRel) && LOAD_DONE.load(Ordering::Acquire) {
        kbd_load("prefchanged");
        if is_open() && cur_tab() == KB_TAB { repaint(); }
    }
    let owed = KB_PERSIST.swap(0, Ordering::AcqRel);
    if owed == 0 { return; }
    use crate::prefs::{key, PrefValue as P}; use crate::prefs_client::sys_set as set_sys;
    if owed & 1 != 0 { set_sys(key::KBD_LAYOUT, P::Str(String::from(kl::active().name))); }
    if owed & 2 != 0 { set_sys(key::KBD_COMPOSE, P::Bool(kl::compose_on())); }
}

fn paint_keyboard(st: &mut State) {
    use crate::video::keymap::layouts as kl;
    groups(st, &[(0, 1)]);
    let l = kl::active();
    let on = kl::compose_on();
    let labels: [&[u8]; 8] = core::array::from_fn(|k| kl::LAYOUTS[k].label.as_bytes());
    txt(st, LABEL_X, 0, "Layout");
    seg(st, 0, &labels, kl::index_of(l.name).unwrap_or(0), false);
    txt2(st, VAL_X, 0, l.title);
    txt(st, LABEL_X, 1, "Compose key");
    tog(st, 1, TOG_X, on, false);
    txt2(st, TRACK_X, 1, if on { "Tap Right Alt, then two keys: s s, = e, \" u" } else { "Right Alt is AltGr" });
    txt2(st, LABEL_X, 3, &alloc::format!("{}: {} dead keys. Press one, then a letter; press it twice for the mark itself.", l.title, kl::dead_count(l)));
    txt2(st, LABEL_X, 4, "Caps Lock shifts letters only; Right Alt held with a key types its AltGr character.");
}

/// A press on the Keyboard tab: a layout segment, or the Compose switch (anywhere on its row).
fn press_keyboard(row: usize, cx: usize) {
    use crate::video::keymap::layouts as kl;
    let n = kl::LAYOUTS.len();
    match row {
        0 => { if cx >= TRACK_X && cx < TRACK_X + TRACK_W { kbd_set(0, ((cx - TRACK_X) / (TRACK_W / n)).min(n - 1)); } }
        1 => kbd_set(1, (!kl::compose_on()) as usize),
        _ => {}
    }
}

// ── SMARTLINE (rmbp-ledger B708, BP-bar-1) — the Storage tab ─────────────────────────────────────────────────────
// One card (a group box of three rows) per disk the AHCI probe published, at most STORAGE_CARDS and `+<n> more`:
// row 0 the model (bold), size, firmware and TRIM; row 1 power-on hours, wear, temperature, power cycles; row 2 the
// verdict — `Health: OK` (ink.positive), `Health: WARN - <attribute> <value>` (ink.warning), or `Health: not reported`
// with SMART's reason. Read-only: the pane edits no key (no MAP row, BP-video-16) and has no press arm. The data is
// `drivers::ahci::smart`'s boot snapshot, copied under `try_lock` — the render task never issues a disk command.

/// Cards the pane shows (three rows each, inside ROWS).
const STORAGE_CARDS: usize = if cfg!(all(target_arch = "x86_64", feature = "wc", feature = "unafs")) { 2 } else { 3 }; // GEODEBACKUP (B740): rows 7..9 are the backup group under the knob

/// One disk's card, as text: the head, the facts, the verdict and its level (0 not reported, 1 OK, 2 WARN).
struct StorageCard {
    head: String,
    facts: String,
    health: String,
    level: u8,
}

/// The cards, from the AHCI SMART snapshot (none on a build without the driver).
fn storage_cards() -> Vec<StorageCard> {
    #[cfg(all(target_arch = "x86_64", feature = "ahci"))]
    {
        use crate::drivers::ahci::smart::{self, Health, Smart};
        smart::disks().iter().map(|d| {
            let bytes = d.ident.sectors.saturating_mul(512);
            let size = if bytes >= 1_000_000_000_000 {
                alloc::format!("{}.{} TB", bytes / 1_000_000_000_000, (bytes / 100_000_000_000) % 10)
            } else {
                alloc::format!("{} GB", bytes / 1_000_000_000)
            };
            let fw = d.ident.firmware();
            let head = alloc::format!("{}  -  {}{}{}", d.ident.model(), size,
                if fw.is_empty() { String::new() } else { alloc::format!("  -  firmware {}", fw) },
                if d.ident.trim { "  -  TRIM" } else { "" });
            let s = &d.sum;
            let mut facts: Vec<String> = Vec::new();
            if let Some(h) = s.hours { facts.push(alloc::format!("{} h powered on", h)); }
            if let Some(w) = s.wear_pct { facts.push(alloc::format!("wear {}%", w)); }
            if let Some(c) = s.temp_c { facts.push(alloc::format!("{} C", c)); }
            if let Some(n) = s.cycles { facts.push(alloc::format!("{} power cycles", n)); }
            let facts = if facts.is_empty() { String::from("No health counters reported") } else { facts.join("  -  ") };
            let (health, level) = match (&d.smart, s.health) {
                (_, Health::Ok) => (String::from("Health: OK"), 1),
                (_, Health::Warn(t)) => (alloc::format!("Health: WARN - {}", t.label()), 2),
                (Smart::Unsupported(why), Health::Na) => (alloc::format!("Health: not reported - SMART {}", why), 0),
                (Smart::Read(..), Health::Na) => (String::from("Health: not reported"), 0),
            };
            StorageCard { head, facts, health, level }
        }).collect()
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "ahci")))]
    {
        Vec::new()
    }
}

fn paint_storage(st: &mut State) {
    let cards = storage_cards();
    if cards.is_empty() {
        groups(st, &[(0, 0)]);
        txt2(st, LABEL_X, 0, "No disk on this machine reports its health (SATA disks are read at startup)");
        return;
    }
    let shown = cards.len().min(STORAGE_CARDS);
    let g: Vec<(usize, usize)> = (0..shown).map(|i| (i * 3, i * 3 + 2)).collect();
    groups(st, &g);
    let face = super::text::Face::Ui;
    let ch = super::metrics::lcell_h(face);
    for (i, c) in cards.iter().take(shown).enumerate() {
        let r = i * 3;
        text_at(st, LABEL_X, row_top(r) + (ROW_H - ch) / 2, CTRL_R, &c.head, theme::text_primary(), true, face);
        txt2(st, LABEL_X, r + 1, &c.facts);
        let ink = match c.level { 1 => theme::ink_positive(), 2 => theme::ink_warning(), _ => theme::ink_secondary() };
        txt_ink(st, LABEL_X, r + 2, &c.health, ink);
    }
    if cards.len() > shown {
        txt2(st, LABEL_X, if cfg!(all(target_arch = "x86_64", feature = "wc", feature = "unafs")) { STORAGE_CARDS * 3 } else { ROWS - 1 }, &alloc::format!("+{} more", cards.len() - shown)); // GEODEBACKUP (B740): above the backup group
    }
}
// ── AUTOLOCK (rmbp-ledger B712, BP-bar-15/20) — the Security tab — TAIL-APPENDED, so no line above moves ──────────────
// Three MAP rows, one per `system.lock.*` key: `Lock after idle` (a segmented control over `lock::IDLE_STEPS`: Never, 5,
// 10, 30, 60 minutes), `Lock when the lid closes` and `Lock before sleep` (switches). A press applies at once through
// `video::lock::set_policy` (the lock reads it on its next pass) and owes the store write, made by
// [`security_service`] on the service pass (`lock::persist`, never in the router). A fourth line says what locks now.

/// The Security tab's index on the strip (14 since the fold); Hardware (HWTAB B737) follows it, the last.
pub const SEC_TAB: usize = 14;
/// The three rows' labels, as painted and as MAP names them.
const SEC_ROW_NAMES: [&str; 3] = ["Lock after idle", "Lock when the lid closes", "Lock before sleep"];
/// The idle steps' segment labels (`lock::IDLE_STEPS`' order).
const SEC_IDLE_LABELS: [&[u8]; 5] = [b"Never", b"5 min", b"10 min", b"30 min", b"1 hr"];

/// The policy as the pane shows it: `(after_idle_min, on_lid, on_sleep)` — the lock's own (a press applies there first).
fn security_policy() -> (u32, bool, bool) {
    #[cfg(feature = "login")]
    {
        crate::video::lock::policy()
    }
    #[cfg(not(feature = "login"))]
    {
        (0, false, false)
    }
}

/// The idle step a stored value shows as: the nearest step at or above it (a hand-set 15 shows 30), Never for 0.
fn sec_idle_step(m: u32) -> usize {
    #[cfg(feature = "login")]
    {
        let steps = crate::video::lock::IDLE_STEPS;
        if m == 0 { return 0; }
        steps.iter().position(|&s| s >= m && s != 0).unwrap_or(steps.len() - 1)
    }
    #[cfg(not(feature = "login"))]
    {
        let _ = m;
        0
    }
}

fn paint_security(st: &mut State) {
    let (m, lid, sleep) = security_policy();
    groups(st, &[(0, 2)]);
    txt(st, LABEL_X, 0, SEC_ROW_NAMES[0]);
    seg(st, 0, &SEC_IDLE_LABELS, sec_idle_step(m), false);
    txt(st, LABEL_X, 1, SEC_ROW_NAMES[1]);
    tog(st, 1, TOG_X, lid, false);
    txt(st, LABEL_X, 2, SEC_ROW_NAMES[2]);
    tog(st, 2, TOG_X, sleep, false);
    txt2(st, LABEL_X, 3, if cfg!(feature = "login") { "Win+L locks now. Your password opens it; nothing is closed." } else { "No login on this build: nothing locks." });
}

/// A press on the Security tab: the segment under `cx` on row 0, a switch flip on rows 1 and 2.
fn press_security(row: usize, cx: usize) {
    #[cfg(feature = "login")]
    {
        use crate::video::lock::{set_policy, IDLE_STEPS};
        let (_, lid, sleep) = security_policy();
        match row {
            0 => {
                if cx < TRACK_X || cx >= TRACK_X + TRACK_W { return; }
                let k = ((cx - TRACK_X) / (TRACK_W / IDLE_STEPS.len())).min(IDLE_STEPS.len() - 1);
                set_policy(Some(IDLE_STEPS[k]), None, None);
                say("lock.after_idle_min", &alloc::format!("{}", IDLE_STEPS[k]), true);
            }
            1 => { set_policy(None, Some(!lid), None); say("lock.on_lid", if !lid { "true" } else { "false" }, true); }
            2 => { set_policy(None, None, Some(!sleep)); say("lock.on_sleep", if !sleep { "true" } else { "false" }, true); }
            _ => return,
        }
        repaint();
    }
    #[cfg(not(feature = "login"))]
    {
        let _ = (row, cx);
    }
}

/// The service pass: the Security rows' owed store writes.
fn security_service() {
    #[cfg(feature = "login")]
    crate::video::lock::persist();
}

// ── UPDATEUI (rmbp-ledger B726, BP-bar-8) — About > System image ── TAIL-APPENDED, `installdemo` (x86) only ──────────
// The running image's KELF verdict, the staged image (its build sha, when, from where) or a set-aside one, and ONE
// button: `Stage an image from…` (it lists the removable volumes carrying an image and its `SRC.SHA`; a press on one
// stages it through `install::abimage::stage_from`, the `stage` verb's path) or, after the loader's fallback, `Remove`.
// Nothing restarts on its own (BP-bar-8): the row says what the user's own Restart will do. Every ESP read and write is
// on the settings service pass ([`sysimg::service`]); the painter reads the model the pass left.
#[cfg(all(target_arch = "x86_64", feature = "installdemo"))]
pub(crate) mod sysimg {
    use super::{btn_at, groups, mid_y, txt, txt2, txt_ink, State, BTN_H, LABEL_X, ROWS};
    use crate::install::abimage::{self, Candidate, Status};
    use crate::video::theme;
    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;

    /// The About tab's index in `TAB_NAMES`.
    const ABOUT: usize = 3;
    /// The first System image row; the card above it ends at `row_top(FIRST)`.
    pub const FIRST: usize = 5;
    /// The compact card's fact pitch is this much tighter than the full card's.
    pub const FACT_TIGHTEN: usize = 6;
    /// Rows: the running image, the staged one, the sentence, the button(s).
    const R_RUN: usize = FIRST;
    const R_STAGED: usize = FIRST + 1;
    const R_NOTE: usize = FIRST + 2;
    const R_BTN: usize = FIRST + 3;
    const _: () = assert!(R_BTN < ROWS - 1); // the last row is the pane's file footer
    /// The value column (right of the labels).
    const VAL: usize = LABEL_X + 140;
    /// The button widths: the stage button, one per listed volume, Cancel / Remove.
    const STAGE_W: usize = 200;
    const VOL_W: usize = 150;
    const SMALL_W: usize = 100;
    /// At most this many volumes are offered in the row.
    const VOL_MAX: usize = 3;

    pub const NOTE_STAGED: &str = "Restart to try it — falls back by itself if it does not reach the desktop";
    pub const NOTE_NONE_FOUND: &str = "No removable volume carries an UnaOS image with its SRC.SHA";

    /// What the service pass owes.
    enum Owed {
        None,
        Stage(String),
        Remove,
    }

    struct Model {
        st: Option<Status>,
        cands: Vec<Candidate>,
        /// The volume list is open in the button row.
        picking: bool,
        /// The status was read since the tab became visible.
        fresh: bool,
        owed: Owed,
        /// The last action's outcome (a refusal), shown in the sentence row when nothing else is.
        said: String,
    }

    static M: crate::sync::Mutex<Model> = crate::sync::Mutex::new(Model { st: None, cands: Vec::new(), picking: false, fresh: false, owed: Owed::None, said: String::new() });

    /// The compact card's mark size (none: the System image rows need its height).
    pub fn card_mark(_full: usize) -> usize {
        0
    }

    /// The three text rows (running, staged, sentence) from `st`; one fn for the painter and `tests updateui`.
    pub fn text_of(st: Option<&Status>, picking: bool, cands: usize, said: &str) -> [String; 3] {
        let Some(st) = st else { return [String::from("reading…"), String::new(), String::new()] };
        let run = match &st.running {
            Some(r) => format!("slot {} · hash {} · sha256 {}", r.slot, if r.hash_ok { "ok" } else { "unverified" }, r.sha),
            None => String::from("the loader handed over no verdict"),
        };
        let (staged, note) = if let Some(s) = &st.staged {
            let when = if s.at == 0 { String::from("time unknown") } else { crate::clock::locale::fmt_stamp_in(&crate::clock::locale::current(), s.at) };
            (format!("{} from {} · {}", s.sha, s.src, when), String::from(NOTE_STAGED))
        } else if let Some(n) = st.set_aside {
            let boot = if n == 0 { String::from("boot unknown") } else { format!("boot {}", n) };
            (String::from("set aside"), format!("The staged image did not reach the desktop and was set aside ({})", boot))
        } else {
            (String::from("nothing staged"), String::from(said))
        };
        let note = if picking && cands == 0 && st.set_aside.is_none() { String::from(NOTE_NONE_FOUND) } else { note };
        [run, staged, note]
    }

    /// The rows: the group box, the labels and values, the button row.
    pub(super) fn paint(st: &mut State) {
        let (lines, picking, aside, vols) = {
            let m = M.lock();
            let t = text_of(m.st.as_ref(), m.picking, m.cands.len(), &m.said);
            let aside = m.st.as_ref().is_some_and(|s| s.set_aside.is_some());
            (t, m.picking, aside, m.cands.iter().take(VOL_MAX).map(|c| format!("{} ({})", c.vol, c.sha)).collect::<Vec<_>>())
        };
        groups(st, &[(R_RUN, R_BTN)]);
        txt(st, LABEL_X, R_RUN, "System image");
        txt2(st, VAL, R_RUN, &lines[0]);
        txt(st, LABEL_X, R_STAGED, "Staged");
        txt2(st, VAL, R_STAGED, &lines[1]);
        if aside { txt_ink(st, LABEL_X, R_NOTE, &lines[2], theme::text_primary()); } else { txt2(st, LABEL_X, R_NOTE, &lines[2]); }
        let y = mid_y(R_BTN, BTN_H);
        if aside {
            btn_at(st, LABEL_X, y, SMALL_W, "Remove", false, false);
        } else if picking {
            for (k, v) in vols.iter().enumerate() {
                btn_at(st, LABEL_X + k * (VOL_W + 10), y, VOL_W, v, false, false);
            }
            btn_at(st, LABEL_X + vols.len() * (VOL_W + 10), y, SMALL_W, "Cancel", false, false);
        } else {
            btn_at(st, LABEL_X, y, STAGE_W, "Stage an image from…", false, false);
        }
    }

    /// A press in the About pane (logical row and x): only the button row acts; the action is owed to [`service`].
    pub(super) fn press(row: usize, cx: usize) {
        if row != R_BTN || cx < LABEL_X {
            return;
        }
        let x = cx - LABEL_X;
        {
            let mut m = M.lock();
            let aside = m.st.as_ref().is_some_and(|s| s.set_aside.is_some());
            if aside {
                if x < SMALL_W { m.owed = Owed::Remove; }
            } else if m.picking {
                let n = m.cands.len().min(VOL_MAX);
                let k = x / (VOL_W + 10);
                if k < n && x % (VOL_W + 10) < VOL_W {
                    let v = m.cands[k].vol.clone();
                    m.owed = Owed::Stage(v);
                    m.picking = false;
                } else if k == n && x % (VOL_W + 10) < SMALL_W {
                    m.picking = false;
                }
            } else if x < STAGE_W {
                m.picking = true;
                m.said.clear();
                m.fresh = false; // the list is read again on the pass
            }
        }
        super::repaint();
    }

    /// The pane's choice of a volume (the press's latch; `tests updateui` names one directly).
    pub fn pick(vol: &str) {
        M.lock().owed = Owed::Stage(String::from(vol));
    }

    /// The pane's Remove (the press's latch; `tests updateui` presses it directly).
    pub fn remove() {
        M.lock().owed = Owed::Remove;
    }

    /// Run what is owed against `src` (the namespace, or the fixture's RAM volumes), then read the status and list
    /// `vols` (`None` = the removable volumes mounted now).
    pub fn run_in(src: &crate::fs::vfs::MountTable, vols: Option<&[String]>) {
        let owed = core::mem::replace(&mut M.lock().owed, Owed::None);
        let said = match owed {
            Owed::Stage(v) => Some(match abimage::stage_from_in(src, &v) {
                Ok(_) => String::new(),
                Err(w) => format!("{} was not staged: {}", v, w),
            }),
            Owed::Remove => Some(match abimage::remove_set_aside() {
                Ok(()) => String::from("The set-aside image was removed"),
                Err(w) => format!("Remove failed: {}", w),
            }),
            Owed::None => None,
        };
        let st = abimage::status();
        let cands = match vols {
            Some(v) => abimage::candidates_in(src, v),
            None => abimage::volumes(),
        };
        let mut m = M.lock();
        m.st = Some(st);
        m.cands = cands;
        m.fresh = true;
        if let Some(s) = said {
            m.said = s;
        }
    }

    /// The text rows as the painter would draw them now.
    pub fn lines() -> [String; 3] {
        let m = M.lock();
        text_of(m.st.as_ref(), m.picking, m.cands.len(), &m.said)
    }

    /// The volumes the list offers (their leaves), and the last action's outcome — what `tests updateui` reads back.
    pub fn listed() -> (Vec<String>, String) {
        let m = M.lock();
        (m.cands.iter().map(|c| c.vol.clone()).collect(), m.said.clone())
    }

    /// Forget the model (the fixture's clean-up): the next visible pass reads the namespace again.
    pub fn stale() {
        let mut m = M.lock();
        m.fresh = false;
        m.picking = false;
        m.said.clear();
    }

    /// The settings service pass: while About is showing, read the status once and run any owed action.
    pub(super) fn service() {
        let vis = super::is_open() && super::cur_tab() == ABOUT;
        let due = {
            let mut m = M.lock();
            if !vis {
                m.fresh = false;
                m.picking = false;
                return;
            }
            !m.fresh || !matches!(m.owed, Owed::None)
        };
        if due {
            run_in(&crate::shell::vfs_mount_table(), None);
            super::repaint();
        }
    }
}
// ── HWREPORT (rmbp-ledger B725, BP-bar-25) -> HWTAB (rmbp-ledger B737) — the Hardware tab — TAIL-APPENDED ─────────────
// The tab (`HW_TAB`, the last): row 0 a group with the census's summary and a `Refresh` button; rows 1..8 the list, one
// line per device `hw::census` found: what it is, its bus, ids and place, the driver's module name and its state; a
// device no driver claimed says `No driver yet`; row 9 the note. The paint reads `hw::last` (`try_lock`, never a
// census, never a register): a missing or stale census (older than [`HW_STALE_MS`]) is asked for, and the settings
// service pass ([`hw_service`]) builds it without waiting (a held USB loan = ask again next pass). The wheel over the
// pane scrolls the list ([`hw_wheel`]); a thumb shows where the visible lines sit. A press on a line copies it to the
// console as `[hw] row <line>` and says on the note line what to do with it ([`hw_press`], BP-bar-25's honest line).

/// HWTAB (B737): the Hardware tab's index on the strip, the last.
pub const HW_TAB: usize = 15;
/// The summary row and the list's first and last rows; the note goes on the row after the list.
const HW_SUM_ROW: usize = 0;
const HW_LIST_ROW: usize = 1;
const HW_NOTE_ROW: usize = ROWS - 1;
/// The Refresh button's x (right-aligned in the summary row).
const HW_REFRESH_X: usize = CTRL_R - BTN_W;
/// A census older than this is asked for again while the tab is shown.
const HW_STALE_MS: u64 = 15_000;
static HW_TOP: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static HW_WANT: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static HW_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The note line: what the last press copied, or the honest line for a device with no driver.
static HW_NOTE: crate::sync::Mutex<String> = crate::sync::Mutex::new(String::new());

/// The list's geometry: `(box top, box bottom, line pitch, visible lines)`, logical px.
fn hw_geom() -> (usize, usize, usize, usize) {
    let uc = super::metrics::lcell_h(super::text::Face::Ui);
    let top = row_top(HW_LIST_ROW) + GAP;
    let bot = row_top(HW_NOTE_ROW) - GAP;
    let lh = uc + 8;
    (top, bot, lh, (bot.saturating_sub(top + 8) / lh).max(1))
}

/// The list's first shown line for `n` lines (HW_TOP clamped to the last page).
fn hw_first(n: usize) -> usize {
    HW_TOP.load(Ordering::Relaxed).min(n.saturating_sub(hw_geom().3))
}

fn paint_hardware(st: &mut State) {
    let c = crate::hw::last();
    let now = crate::arch::ms();
    if c.is_none() || now.saturating_sub(HW_AT.load(Ordering::Relaxed)) >= HW_STALE_MS {
        HW_WANT.store(true, Ordering::Relaxed);
    }
    let face = super::text::Face::Ui;
    let (top, bot, lh, vis) = hw_geom();
    groups(st, &[(HW_SUM_ROW, HW_SUM_ROW)]);
    {
        let mut s = sf(st);
        draw::group_box(&mut s, Rect::new(e(BOX_X), e(top), ew(BOX_X, BOX_W), e(bot) - e(top)), b"");
    }
    btn(st, HW_SUM_ROW, HW_REFRESH_X, "Refresh");
    let note = HW_NOTE.try_lock().map(|g| g.clone()).unwrap_or_default();
    if note.is_empty() {
        txt2(st, LABEL_X, HW_NOTE_ROW, "Press a device to copy its line to the console.");
    } else {
        txt(st, LABEL_X, HW_NOTE_ROW, &note);
    }
    let Some(c) = c else {
        txt2(st, LABEL_X, HW_SUM_ROW, "Reading the devices...");
        return;
    };
    let n = c.rows.len();
    let d = c.driven();
    txt(st, LABEL_X, HW_SUM_ROW, &alloc::format!("{} devices  -  {} with a driver  -  {} without", n, d, n - d));
    let first = hw_first(n);
    HW_TOP.store(first, Ordering::Relaxed);
    let (where_x, drv_x, st_x) = (LABEL_X + 230, CTRL_R - 190, CTRL_R - 96);
    for (k, r) in c.rows.iter().enumerate().skip(first).take(vis) {
        let y = top + 8 + (k - first) * lh;
        text_at(st, LABEL_X, y, where_x - 8, &r.name, theme::text_primary(), false, face);
        text_at(st, where_x, y, drv_x - 8, &alloc::format!("{} {}  {}", r.bus, r.id, r.at), theme::ink_secondary(), false, face);
        if r.driver == "none" {
            text_at(st, drv_x, y, CTRL_R, "No driver yet", theme::ink_secondary(), false, face);
        } else {
            text_at(st, drv_x, y, st_x - 8, r.driver, theme::text_primary(), false, face);
            let ink = if r.state == "up" { theme::ink_positive() } else if r.state.starts_with("refused") { theme::ink_warning() } else { theme::ink_secondary() };
            text_at(st, st_x, y, CTRL_R, r.state, ink, false, face);
        }
    }
    if n > vis {
        // The scroll thumb: where the visible lines sit in the whole list.
        let track = bot - top - 8;
        let (ty, th) = (top + 4 + first * track / n, (vis * track / n).max(8));
        let r = Rect::new(e(BOX_X + BOX_W - 6), e(ty), ew(BOX_X + BOX_W - 6, 3), ew(ty, th));
        draw::fill_rrect(&mut sf(st), r, r.w / 2, Ink::argb(theme::control_edge()));
    }
}

/// A wheel detent over the Hardware pane: scroll the list (positive = toward the top, QSCROLL's sign).
fn hw_wheel(d: i8, step: usize) -> (&'static str, bool) {
    let n = crate::hw::last().map(|c| c.rows.len()).unwrap_or(0);
    let vis = hw_geom().3;
    let cur = HW_TOP.load(Ordering::Relaxed);
    let next = (if d > 0 { cur.saturating_sub(step) } else { cur + step }).min(n.saturating_sub(vis));
    HW_TOP.store(next, Ordering::Relaxed);
    ("hardware", next != cur)
}

/// The census line under the logical `(cx, cy)` in a list of `n`, if a line is painted there.
fn hw_line_at(cy: usize, cx: usize, n: usize) -> Option<usize> {
    let (top, bot, lh, vis) = hw_geom();
    if cx < BOX_X || cx >= BOX_X + BOX_W || cy < top + 8 || cy >= bot {
        return None;
    }
    let i = (cy - top - 8) / lh;
    let k = hw_first(n) + i;
    (i < vis && k < n).then_some(k)
}

/// The note a press on `r` leaves: the honest line for a device no driver claimed (BP-bar-25), else what was copied.
fn hw_note(r: &crate::hw::Row) -> String {
    if r.driver == "none" {
        alloc::format!("No driver yet — tell us: {} {}", r.bus, r.id)
    } else {
        alloc::format!("Copied to the console: {}", r.name)
    }
}

/// A press in the Hardware pane: `Refresh` asks for a census (the service pass builds it); a line copies the device's
/// `hw::line` to the console (the terminal ring, wait-free, and the serial wire) and sets the note.
fn hw_press(row: usize, cy: usize, cx: usize) {
    if row == HW_SUM_ROW {
        if cx >= HW_REFRESH_X && cx < HW_REFRESH_X + BTN_W {
            HW_AT.store(0, Ordering::Relaxed);
            HW_WANT.store(true, Ordering::Relaxed);
            serial_println!("[hw] refresh asked");
        }
        repaint();
        return;
    }
    let Some(c) = crate::hw::last() else { repaint(); return };
    if let Some(k) = hw_line_at(cy, cx, c.rows.len()) {
        let r = &c.rows[k];
        let l = crate::hw::line(r);
        let _ = crate::termring::console_out(format_args!("[hw] row {}", l));
        serial_println!("[hw] row {}", l);
        let note = hw_note(r);
        if let Some(mut g) = HW_NOTE.try_lock() {
            *g = note;
        }
    }
    repaint();
}

/// The service pass: a census the Hardware pane asked for, built without waiting (a held registry asks again next pass).
fn hw_service() {
    if !HW_WANT.load(Ordering::Relaxed) || !is_open() || cur_tab() != HW_TAB {
        return;
    }
    let c = crate::hw::census(false);
    if !c.busy {
        HW_WANT.store(false, Ordering::Relaxed);
        HW_AT.store(crate::arch::ms(), Ordering::Relaxed);
        repaint();
    }
}

/// `tests hwreport`'s tab leg (HWTAB B737): the Hardware tab painted OFFSCREEN (a fresh `State`, the live window
/// untouched) page by page through the list's scroll, and each census line whose band carries ink counted once; then
/// the press leg: the first `none` row (else the first row) scrolled into view, its line's centre resolved back to it
/// by the press router's own [`hw_line_at`], and its note read. HW_TOP is put back. Returns
/// `(tab, listed, rows_painted, press_ok, note)`; `listed` = `hw::last`'s rows (0 when the census is held).
pub fn hwtab_probe() -> (usize, usize, usize, bool, String) {
    let Some(c) = crate::hw::last() else { return (HW_TAB, 0, 0, false, String::from("no-census")) };
    let n = c.rows.len();
    let (pw, ph) = (super::metrics::size(WIN_W), super::metrics::size(WIN_H));
    let mut buf: Vec<u32> = Vec::new();
    if buf.try_reserve_exact(pw * ph).is_err() {
        return (HW_TAB, n, 0, false, String::from("oom"));
    }
    buf.resize(pw * ph, 0);
    let top0 = HW_TOP.load(Ordering::Relaxed);
    let (top, _, lh, vis) = hw_geom();
    let uc = super::metrics::lcell_h(super::text::Face::Ui);
    let mut v = CUR.lock().clone();
    v.tab = HW_TAB as u8;
    let mut seen: Vec<bool> = alloc::vec![false; n];
    let mut page = 0;
    loop {
        HW_TOP.store(page, Ordering::Relaxed);
        let mut st = State::new(WIN_W, WIN_H, core::mem::take(&mut buf), 1, true, 0);
        paint(&mut st, &v);
        buf = core::mem::take(&mut st.surf);
        let first = hw_first(n);
        let px = |x: i32, y: i32| buf.get(y as usize * pw + x as usize).map_or(u32::MAX, |p| p & 0x00FF_FFFF);
        for (i, k) in (first..n).take(vis).enumerate() {
            let y = top + 8 + i * lh;
            let (y0, y1) = (e(y), e(y + uc));
            let ground = px(e(BOX_X + 2), y0);
            let inked = (y0..y1).any(|yy| (e(LABEL_X)..e(LABEL_X + 200)).any(|xx| px(xx, yy) != ground));
            if inked {
                seen[k] = true;
            }
        }
        if first + vis >= n {
            break;
        }
        page = first + vis;
    }
    let painted = seen.iter().filter(|&&b| b).count();
    let k = c.rows.iter().position(|r| r.driver == "none").unwrap_or(0);
    let (press, note) = if n == 0 {
        (false, String::from("empty"))
    } else {
        HW_TOP.store(k, Ordering::Relaxed);
        let i = k - hw_first(n);
        let hit = hw_line_at(top + 8 + i * lh + lh / 2, LABEL_X + 4, n);
        let note = hw_note(&c.rows[k]);
        let want = if c.rows[k].driver == "none" { note.starts_with("No driver yet — tell us: ") } else { note.starts_with("Copied") };
        (hit == Some(k) && want, note)
    };
    HW_TOP.store(top0, Ordering::Relaxed);
    (HW_TAB, n, painted, press, note)
}

/// crawl A 6 (BP-video-25) — TAIL-APPENDED. The colour filter moved: every scanout writer filters at its copy, so
/// everything already on the glass is in the OLD colours until it is copied again — bump the theme epoch (every strip
/// re-seals, `strip::seal`), damage every window and ask the desktop layer for its full present, as `set_contrast`'s
/// epoch bump and the lock's wipe do.
fn a11y_filter_moved() {
    super::theme::bump_epoch();
    let _ = wm::damage_intersecting(0, 0, 1 << 16, 1 << 16);
    super::screen::request_full_present();
}
// ── GEODEBACKUP (rmbp-ledger B740, BP-bar-1/14, BP-video-37) — Storage > Back up / Restore — TAIL-APPENDED ─────────────
// Under the disk cards (two under this knob), three rows: `Back up Home to…` (it lists the writable removable volumes
// whose free space holds the home's `.geode` archive, measured), `Restore from…` (it lists the `*.geode` files at the
// sticks' roots; a pick unpacks into `<home>/Restored <stem>`, a taken name refused), and the note row (`Backing up:
// 12 of 340 files`, then the outcome). The press only latches; the settings service pass submits the job to the
// `geode` worker task (`fs::unafs::geodew::job`), never running it on the render task; the painter reads the job's
// atomics and its model by `try_lock`. Versions are not carried (v1): the note says so after a backup.
#[cfg(all(target_arch = "x86_64", feature = "wc", feature = "unafs"))]
pub(crate) mod backup {
    use super::{btn_at, groups, mid_y, txt2, txt_ink, State, BTN_H, LABEL_X, ROWS};
    use crate::fs::unafs::geodew::job::{self, Req, BACKING_UP, MEASURING, RESTORING};
    use crate::video::theme;
    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;
    use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

    /// The rows: Back up, Restore, the note (the pane's last three).
    const R_BACK: usize = ROWS - 3;
    const R_REST: usize = ROWS - 2;
    const R_NOTE: usize = ROWS - 1;
    const OPEN_W: usize = 200;
    const PICK_W: usize = 170;
    const SMALL_W: usize = 100;
    /// At most this many volumes / archives are offered in a row.
    const PICK_MAX: usize = 3;

    /// Which list is open: 0 none, 1 the backup volumes, 2 the archives.
    static PICK: AtomicU8 = AtomicU8::new(0);
    /// What the press owes the service pass: 0 none, 1 survey, 2 back up to pick `ARG`, 3 restore pick `ARG`.
    static OWED: AtomicU8 = AtomicU8::new(0);
    static ARG: AtomicU8 = AtomicU8::new(0);
    /// The job's `(generation, phase, done)` the pane last painted.
    static SEEN: AtomicU64 = AtomicU64::new(u64::MAX);

    /// The note row's text from the job's progress and its last outcome (one fn for the painter and the fixture).
    pub fn note_of(phase: u8, done: u32, total: u32, said: &str) -> String {
        match phase {
            MEASURING => String::from("Measuring Home and the volumes…"),
            BACKING_UP => format!("Backing up: {} of {} files", done, total),
            RESTORING if total > 0 => format!("Restoring: {} of {} files", done, total),
            RESTORING => String::from("Checking the archive…"),
            _ => String::from(said),
        }
    }

    /// The session's home as the mount table names it.
    fn home() -> Option<(String, String)> {
        #[cfg(feature = "login")]
        {
            let mut b = [0u8; 32];
            let n = crate::arch::x86_64::syscall::session_name(&mut b)?;
            let mut h = [0u8; crate::fs::users::HOME_MAX];
            let k = crate::fs::users::home_of(&b[..n], &mut h)?;
            let user = core::str::from_utf8(&b[..n]).ok()?;
            let home = core::str::from_utf8(&h[..k]).ok()?;
            return Some((String::from(user), String::from(home)));
        }
        #[allow(unreachable_code)]
        None
    }

    fn mb(b: u64) -> u64 {
        (b + 999_999) / 1_000_000
    }

    pub(super) fn paint(st: &mut State) {
        let (phase, done, total, _) = job::progress();
        let pick = PICK.load(Ordering::Relaxed);
        let (vols, archives, need, surveyed, said) = match job::model() {
            Some(m) => (
                m.vols.iter().take(PICK_MAX).map(|(v, f)| format!("{} ({} MB free)", v, mb(*f))).collect::<Vec<_>>(),
                m.archives.iter().take(PICK_MAX).map(|(v, f)| format!("{}: {}", v, f)).collect::<Vec<_>>(),
                m.need,
                m.surveyed,
                m.said.clone(),
            ),
            None => (Vec::new(), Vec::new(), 0, false, String::new()),
        };
        groups(st, &[(R_BACK, R_NOTE)]);
        let busy = phase != job::IDLE;
        let picker = |st: &mut State, r: usize, items: &[String], empty: &str| {
            let y = mid_y(r, BTN_H);
            if !surveyed {
                txt2(st, LABEL_X, r, "Measuring…");
                return;
            }
            if items.is_empty() {
                txt2(st, LABEL_X + SMALL_W + 10, r, empty);
            }
            for (k, it) in items.iter().enumerate() {
                btn_at(st, LABEL_X + k * (PICK_W + 10), y, PICK_W, it, false, false);
            }
            let x = if items.is_empty() { LABEL_X } else { LABEL_X + items.len() * (PICK_W + 10) };
            btn_at(st, x, y, SMALL_W, "Cancel", false, false);
        };
        if pick == 1 && !busy {
            let empty = if need == 0 { String::from("Home could not be measured") } else { format!("No removable volume has room for Home ({} MB)", mb(need)) };
            picker(st, R_BACK, &vols, &empty);
        } else {
            btn_at(st, LABEL_X, mid_y(R_BACK, BTN_H), OPEN_W, "Back up Home to…", false, false);
        }
        if pick == 2 && !busy {
            picker(st, R_REST, &archives, "No removable volume carries a .geode archive");
        } else {
            btn_at(st, LABEL_X, mid_y(R_REST, BTN_H), OPEN_W, "Restore from…", false, false);
        }
        let note = note_of(phase, done, total, &said);
        if busy { txt_ink(st, LABEL_X, R_NOTE, &note, theme::text_primary()); } else { txt2(st, LABEL_X, R_NOTE, &note); }
    }

    /// A press in the Storage pane: only the two button rows act; the work is owed to [`service`].
    pub(super) fn press(row: usize, cx: usize) {
        if (row != R_BACK && row != R_REST) || cx < LABEL_X || job::progress().0 != job::IDLE {
            return;
        }
        let x = cx - LABEL_X;
        let want = if row == R_BACK { 1 } else { 2 };
        let pick = PICK.load(Ordering::Relaxed);
        if pick != want {
            if x < OPEN_W {
                PICK.store(want, Ordering::Relaxed);
                job::stale();
                OWED.store(1, Ordering::Release);
            }
        } else {
            let n = job::model().map(|m| if want == 1 { m.vols.len() } else { m.archives.len() }.min(PICK_MAX)).unwrap_or(0);
            let k = x / (PICK_W + 10);
            if n > 0 && k < n && x % (PICK_W + 10) < PICK_W {
                ARG.store(k as u8, Ordering::Relaxed);
                OWED.store(if want == 1 { 2 } else { 3 }, Ordering::Release);
                PICK.store(0, Ordering::Relaxed);
            } else if (n == 0 && x < SMALL_W) || (n > 0 && k == n && x % (PICK_W + 10) < SMALL_W) {
                PICK.store(0, Ordering::Relaxed);
            }
        }
        super::repaint();
    }

    /// The settings service pass: submit what the press owes; repaint while the job moves.
    pub(super) fn service() {
        let owed = OWED.swap(0, Ordering::AcqRel);
        if owed != 0 {
            let r = match home() {
                None => Err("no-session"),
                Some((user, home)) => {
                    let k = ARG.load(Ordering::Relaxed) as usize;
                    let req = match owed {
                        1 => Some(Req::Survey { home }),
                        2 => job::model().and_then(|m| m.vols.get(k).map(|(v, _)| v.clone())).map(|vol| Req::Backup { home, user, vol }),
                        _ => job::model().and_then(|m| m.archives.get(k).cloned()).map(|(vol, file)| Req::Restore { home, vol, file }),
                    };
                    match req {
                        Some(q) => job::submit(q),
                        None => Err("gone"),
                    }
                }
            };
            if let Err(w) = r {
                serial_println!("[backup] ask={} -> refused:{}", owed, w);
                if let Some(mut m) = job::model() {
                    m.said = format!("Not started: {}", w);
                    m.surveyed = true;
                }
                PICK.store(0, Ordering::Relaxed);
            }
            super::repaint();
        }
        if !super::is_open() || super::cur_tab() != super::STORAGE_TAB {
            return;
        }
        let (phase, done, _, g) = job::progress();
        let key = ((g as u64) << 40) | ((phase as u64) << 32) | done as u64;
        if SEEN.swap(key, Ordering::Relaxed) != key {
            super::repaint();
        }
    }
}
