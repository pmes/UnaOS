// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — wm (SOFTY B629: the Start menu and the calendar — two flyouts above the taskbar)
//!
//! Each flyout is a chromeless, NON-modal wm window (`wm::overlay_open`, the Launcher's row shape, never the modal
//! slot the login screen and the alerts own), so the compositor owns its occlusion and its vacate. Its surface is
//! allocated when it OPENS (a refusal is a witnessed `REFUSED`) and painted through the `softy::paint` facade; the
//! paint pass itself allocates nothing.
//!
//! THE START MENU — floating [`k::FLY_GAP`] above the taskbar, centred: a search field (it opens UnaOS's Launcher,
//! which IS the search), "Pinned" and a grid of every table app this build carries (`dock::DP_PINS` through the
//! dock's getters), and a footer with the user and a power button whose menu offers Sleep, Restart and Shut down
//! through the SAME doors the SHARD menu uses (`crystal::power_fire`, `dialog::power_confirm`). A grid press
//! launches DIRECTLY (`dock::launch_named`, R115).
//! THE NOTIFICATION CALENDAR — over the clock: notify's summary (unread, Do Not Disturb), the long date, the month
//! and its grid with today in the accent.
//! QUICK SETTINGS (SOFTY2) — over the tray: the network tile, the sound row (mute, the level slider through
//! `status::set_volume`), the battery and a gear that opens Settings.
//!
//! SOFTY2 (B641): each flyout is a FRAMED surface — the body inside a `FLY_GAP` band that carries its `elev.2` shadow
//! over the wallpaper beneath (`paint::under`, `paint::shadow`), so the corners are real and the flyout lifts.
//!
//! Opened by the Start button / the clock / the tray (`by=button`) or the WINDOWS KEY (`by=key`: a tap of Option on
//! a Mac keyboard, Win on a PC one — R126, counted by `keymap`, acted on in [`service`]); closed by a press outside
//! (`by=outside`), Esc (`by=key`, `keymap`'s Esc edge), a pick (`by=pick`) or a look switch (`by=switch`). Hover
//! repaints ride the look's service pass ([`service`]), never the input router.
//!
//! Wire: `[softy] start open|close by=<button|key|outside|pick|switch|search>` (`[softy] calendar …`, `[softy] quick …`),
//! `[softy] start key role=<option|win> taps=<n> -> open|close`, `[softy] quick volume level=<n>/16 muted=<0|1> …`.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU8, Ordering};

use super::paint::{self as p, Surf};
use super::palette as k;
use crate::video::{dock, text::Face, wm};

/// 0 closed, 1 the Start menu, 2 the calendar, 3 quick settings (SOFTY2).
static KIND: AtomicU8 = AtomicU8::new(0);

struct St {
    kind: u8,
    win: wm::WinId,
    /// The BODY's panel rect (presses and hover are judged against it).
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    /// SOFTY2: the shadow band around the body (the surface is `w + 2m` x `h + 2m`, its window at `(x - m, y - m)`).
    m: usize,
    hot: u16,
    power: bool,
    minute: u64,
    /// SOFTY2: the appearance epoch the surface was painted at (a dark / accent change repaints it).
    epoch: u32,
    buf: Vec<u32>,
}

impl St {
    /// The framed surface over the buffer, and the panel origin of its top-left pixel.
    fn surf(&mut self) -> (Surf<'_>, usize, usize) {
        let (bw, bh, m) = (self.w + 2 * self.m, self.h + 2 * self.m, self.m);
        let (ox, oy) = (self.x - m, self.y - m);
        (Surf::framed(&mut self.buf, bw, bh, m), ox, oy)
    }
}

static ST: crate::sync::Mutex<Option<St>> = crate::sync::Mutex::new(None);

/// Hover / press targets inside the Start menu.
const T_NONE: u16 = 0;
const T_SEARCH: u16 = 1;
const T_GRID: u16 = 100; // + item
const T_POWER: u16 = 200;
const T_POWER_ROW: u16 = 210; // + row

const POWER_ROWS: [&str; 3] = ["Sleep", "Restart", "Shut down"];
const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const DAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const DAY_HEAD: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];

/// Which flyout is open (0 none, 1 Start, 2 the calendar).
pub fn open_kind() -> u8 {
    KIND.load(Ordering::Relaxed)
}

fn word(kind: u8) -> &'static str {
    match kind {
        2 => "calendar",
        3 => "quick",
        _ => "start",
    }
}

/// The table apps the grid shows (every one this build carries), in table order.
fn grid_items(out: &mut [u8; 16]) -> usize {
    let mut n = 0;
    for i in 0..dock::DP_PINS.len() {
        if (i != 2 || cfg!(feature = "quarry")) && n < out.len() {
            out[n] = i as u8;
            n += 1;
        }
    }
    n
}

/// A display label for a table app: its name with a capital.
fn label(pin: usize, buf: &mut [u8; 16]) -> usize {
    let n = dock::DP_PINS[pin].name.as_bytes();
    let l = n.len().min(16);
    buf[..l].copy_from_slice(&n[..l]);
    if l > 0 {
        buf[0] = buf[0].to_ascii_uppercase();
    }
    l
}

// ── geometry (surface-local physical px) ────────────────────────────────────────────────────────────────────────

struct Geo {
    pad: usize,
    search: p::Rect,
    head_y: usize,
    grid_x: usize,
    grid_y: usize,
    cols: usize,
    cw: usize,
    chh: usize,
    foot_y: usize,
    power: p::Rect,
    menu: p::Rect,
}

fn geo(w: usize, h: usize) -> Geo {
    let pad = k::px(k::START_PAD).min(w / 8);
    let search = (pad, pad, w - 2 * pad, k::px(k::SEARCH_H));
    let head_y = search.1 + search.3 + k::px(24);
    let (cw, chh) = (k::px(k::CELL_W), k::px(k::CELL_H));
    let cols = k::GRID_COLS.min((w - 2 * pad) / cw).max(1);
    let grid_x = (w - cols * cw) / 2;
    let grid_y = head_y + Face::Ui.cell_h() + k::px(12);
    let foot_y = h.saturating_sub(k::px(k::FOOT_H));
    let pb = k::px(k::POWER_BTN);
    let power = (w - pad - pb, foot_y + (k::px(k::FOOT_H) - pb) / 2, pb, pb);
    let (mw, mh) = (k::px(k::POWER_W), POWER_ROWS.len() * k::px(k::POWER_ROW_H) + k::px(8));
    let menu = (w - pad - mw, foot_y.saturating_sub(mh + k::px(4)), mw, mh);
    Geo { pad, search, head_y, grid_x, grid_y, cols, cw, chh, foot_y, power, menu }
}

fn inside(r: p::Rect, x: usize, y: usize) -> bool {
    x >= r.0 && y >= r.1 && x < r.0 + r.2 && y < r.1 + r.3
}

/// The Start menu's target under surface-local `(x, y)`.
fn target(st: &St, x: usize, y: usize) -> u16 {
    let g = geo(st.w, st.h);
    if st.power {
        if inside(g.menu, x, y) {
            let r = (y.saturating_sub(g.menu.1 + k::px(4))) / k::px(k::POWER_ROW_H);
            return T_POWER_ROW + r.min(POWER_ROWS.len() - 1) as u16;
        }
    }
    if inside(g.search, x, y) {
        return T_SEARCH;
    }
    if inside(g.power, x, y) {
        return T_POWER;
    }
    let mut items = [0u8; 16];
    let n = grid_items(&mut items);
    if x >= g.grid_x && y >= g.grid_y && y < g.foot_y {
        let (c, r) = ((x - g.grid_x) / g.cw, (y - g.grid_y) / g.chh);
        let i = r * g.cols + c;
        if c < g.cols && i < n {
            return T_GRID + i as u16;
        }
    }
    T_NONE
}

// ── painting ──────────────────────────────────────────────────────────────────────────────────────────────────

/// The flyout's body over what lies beneath: the wallpaper under the band (`p::under`), the `elev.2` shadow, then
/// the rounded body (SOFTY2: real corners and a shadow, where they were the desktop colour cut square before).
fn body(s: &mut Surf, w: usize, h: usize, foot: Option<usize>, org: (usize, usize), m: usize) {
    let r = k::px(k::FLY_R);
    p::under(s, org.0, org.1);
    p::shadow(s, (0, 0, w, h), r, m);
    match foot {
        Some(fy) => {
            p::rrect(s, (0, 0, w, h), r, k::FLYOUT_FOOT(), 255);
            p::rrect_top(s, (0, 0, w, fy), r, k::FLYOUT(), 255);
            p::hairline_h(s, 0, fy, w, k::STROKE(), k::STROKE_A());
        }
        None => p::rrect(s, (0, 0, w, h), r, k::FLYOUT(), 255),
    }
    p::stroke_rrect(s, (0, 0, w, h), r, k::STROKE(), k::STROKE_A());
}

fn paint_start(st: &mut St) {
    let (w, h, hot, power, m) = (st.w, st.h, st.hot, st.power, st.m);
    let g = geo(w, h);
    let (mut s, ox, oy) = st.surf();
    let s = &mut s;
    body(s, w, h, Some(g.foot_y), (ox, oy), m);
    let ui = Face::Ui;
    let ch = ui.cell_h();
    // The search field: white, a hairline, the darker underline that says "type here", a magnifier, a placeholder.
    let sr = g.search;
    let rad = sr.3 / 2;
    p::rrect(s, sr, rad, k::FIELD(), 255);
    p::stroke_rrect(s, sr, rad, k::STROKE(), if hot == T_SEARCH { 2 * k::STROKE_A() } else { k::STROKE_A() });
    p::hairline_h(s, sr.0 + rad, sr.1 + sr.3 - 1, sr.2 - 2 * rad, k::FIELD_LINE(), 255);
    let gl = k::px(12);
    let (gx, gy) = (sr.0 + rad, sr.1 + (sr.3 - gl) / 2);
    p::stroke_rrect(s, (gx, gy, gl * 3 / 4, gl * 3 / 4), gl * 3 / 8, k::TEXT_2(), 255);
    p::line(s, gx + gl * 5 / 8, gy + gl * 5 / 8, gx + gl, gy + gl, k::px(1), k::TEXT_2(), 255);
    let tx = gx + gl + k::px(10);
    p::text(s, tx, sr.1 + (sr.3 - ch) / 2, sr.0 + sr.2 - rad - tx, b"Search apps, settings and files", k::TEXT_2(), false, ui);
    // "Pinned".
    p::text(s, g.pad + k::px(8), g.head_y, w / 2, b"Pinned", k::TEXT(), true, ui);
    // The grid.
    let mut items = [0u8; 16];
    let n = grid_items(&mut items);
    let isz = k::px(k::GRID_ICON);
    for i in 0..n {
        let (c, r) = (i % g.cols, i / g.cols);
        let (x, y) = (g.grid_x + c * g.cw, g.grid_y + r * g.chh);
        if y + g.chh > g.foot_y {
            break;
        }
        let cell = (x + k::px(2), y + k::px(2), g.cw - k::px(4), g.chh - k::px(4));
        if hot == T_GRID + i as u16 {
            p::rrect(s, cell, k::px(k::CELL_R), k::HOVER(), 255);
            p::stroke_rrect(s, cell, k::px(k::CELL_R), k::STROKE(), k::STROKE_A());
        }
        let pin = items[i] as usize;
        let (ix, iy) = (x + (g.cw - isz) / 2, y + k::px(14));
        super::taskbar::tile_icon(s, pin as u8, b"", ix, iy, isz);
        let mut lb = [0u8; 16];
        let ll = label(pin, &mut lb);
        let lw = p::text_w(&lb[..ll], false, ui).min(g.cw - k::px(8));
        p::text(s, x + (g.cw - lw) / 2, iy + isz + k::px(8), g.cw - k::px(8), &lb[..ll], k::TEXT(), false, ui);
    }
    // The footer: the user, the power button.
    let av = k::px(k::AVATAR);
    let (ax, ay) = (g.pad + k::px(8), g.foot_y + (k::px(k::FOOT_H) - av) / 2);
    p::disc(s, ax, ay, av, k::ACCENT(), 255);
    let mut wb = [0u8; crate::prefs::WHO_BUF];
    let who = crate::prefs::user_name_in(&mut wb).map(|n| n.as_bytes()).unwrap_or(&b"UnaOS"[..]);
    let who: &[u8] = if who.is_empty() { b"UnaOS" } else { who };
    let ini = [who[0].to_ascii_uppercase()];
    let iw = p::text_w(&ini, true, ui);
    p::text(s, ax + (av - iw) / 2, ay + (av - ch) / 2, av, &ini, k::ON_ACCENT, true, ui);
    p::text(s, ax + av + k::px(12), g.foot_y + (k::px(k::FOOT_H) - ch) / 2, w / 2, who, k::TEXT(), false, ui);
    let pr = g.power;
    if hot == T_POWER || power {
        p::rrect(s, pr, k::px(k::TILE_R), k::HOVER(), 255);
        p::stroke_rrect(s, pr, k::px(k::TILE_R), k::STROKE(), k::STROKE_A());
    }
    // Our power glyph: a ring open at the top, its stem through the gap.
    let gd = k::px(16);
    let (px0, py0) = (pr.0 + (pr.2 - gd) / 2, pr.1 + (pr.3 - gd) / 2);
    p::stroke_rrect(s, (px0, py0, gd, gd), gd / 2, k::TEXT(), 255);
    p::stroke_rrect(s, (px0 + 1, py0 + 1, gd - 2, gd - 2), (gd - 2) / 2, k::TEXT(), 140);
    let back = if hot == T_POWER || power { p::blend(k::FLYOUT_FOOT(), k::HOVER(), 255) } else { k::FLYOUT_FOOT() };
    p::fill(s, (px0 + gd / 2 - gd / 5, py0, 2 * (gd / 5), gd / 3), back, 255);
    p::line(s, px0 + gd / 2, py0 - k::px(1), px0 + gd / 2, py0 + gd / 2, k::px(1), k::TEXT(), 255);
    // The power menu.
    if power {
        let m = g.menu;
        p::rrect(s, m, k::px(k::FLY_R), k::FIELD(), 255);
        p::stroke_rrect(s, m, k::px(k::FLY_R), k::STROKE(), 2 * k::STROKE_A());
        let rh = k::px(k::POWER_ROW_H);
        for (i, lbl) in POWER_ROWS.iter().enumerate() {
            let ry = m.1 + k::px(4) + i * rh;
            if hot == T_POWER_ROW + i as u16 {
                p::rrect(s, (m.0 + k::px(4), ry, m.2 - k::px(8), rh), k::px(4), k::PRESS(), k::PRESS_A() * 2);
            }
            p::text(s, m.0 + k::px(16), ry + (rh - ch) / 2, m.2 - k::px(24), lbl.as_bytes(), k::TEXT(), false, ui);
        }
    }
}

/// Days in month `mo` (1..=12) of year `y`.
fn month_days(y: i64, mo: u32) -> u32 {
    match mo {
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => 31,
    }
}

fn cal_height() -> usize {
    let ch = Face::Ui.cell_h();
    notes_h() + 2 * k::px(k::CAL_PAD) + ch + k::px(16) + 1 + k::px(16) + ch + k::px(12) + ch + k::px(4) + 6 * k::px(k::CAL_CELL)
}

/// SOFTY2: the notification summary's band at the calendar's head (title, one status line, a divider).
fn notes_h() -> usize {
    let ch = Face::Ui.cell_h();
    2 * ch + k::px(4) + k::px(16) + 1
}

/// SOFTY2 — THE NOTIFICATION CALENDAR's head: what the clock press shows above the month, read from `notify`'s public
/// state (`unread`, `dnd`, `center_open`; nothing in notify changes). Opening notify's own Center from here is a call
/// notify does not export yet (SOFTY2's report names the one line it owes); until then the summary is the bridge.
fn paint_notes(s: &mut Surf, w: usize, pad: usize) -> usize {
    let ui = Face::Ui;
    let ch = ui.cell_h();
    p::text(s, pad, pad, w - 2 * pad, b"Notifications", k::TEXT(), true, ui);
    let n = crate::video::notify::unread();
    let mut buf = [0u8; 48];
    let line: &[u8] = if crate::video::notify::dnd() {
        b"Do not disturb is on"
    } else if n == 0 {
        b"No new notifications"
    } else {
        let mut d = [0u8; 10];
        let mut v = n;
        let mut k = 0;
        while k == 0 || v > 0 {
            d[k] = b'0' + (v % 10) as u8;
            v /= 10;
            k += 1;
        }
        let mut l = 0;
        for i in (0..k).rev() {
            buf[l] = d[i];
            l += 1;
        }
        let tail: &[u8] = if n == 1 { b" new notification" } else { b" new notifications" };
        buf[l..l + tail.len()].copy_from_slice(tail);
        &buf[..l + tail.len()]
    };
    let y = pad + ch + k::px(4);
    if n > 0 && !crate::video::notify::dnd() {
        let dd = k::px(8);
        p::disc(s, pad, y + (ch - dd) / 2, dd, k::ACCENT(), 255);
        p::text(s, pad + dd + k::px(8), y, w - 2 * pad, line, k::TEXT_2(), false, ui);
    } else {
        p::text(s, pad, y, w - 2 * pad, line, k::TEXT_2(), false, ui);
    }
    let div = y + ch + k::px(16) / 2;
    p::hairline_h(s, 0, div, w, k::STROKE(), k::STROKE_A());
    pad + notes_h()
}

fn num(n: u32, out: &mut [u8; 4]) -> usize {
    if n >= 10 {
        out[0] = b'0' + (n / 10 % 10) as u8;
        out[1] = b'0' + (n % 10) as u8;
        2
    } else {
        out[0] = b'0' + n as u8;
        1
    }
}

fn paint_calendar(st: &mut St) {
    let (w, h, m) = (st.w, st.h, st.m);
    let (mut s, ox, oy) = st.surf();
    let s = &mut s;
    body(s, w, h, None, (ox, oy), m);
    let ui = Face::Ui;
    let ch = ui.cell_h();
    let pad0 = k::px(k::CAL_PAD);
    // SOFTY2: the notification summary first; the date and the month below it (`pad` is the month block's top).
    let pad = paint_notes(s, w, pad0);
    let Some(secs) = crate::clock::try_unix_now() else {
        p::text(s, pad0, pad, w - 2 * pad0, b"The clock is not set", k::TEXT(), true, ui);
        p::text(s, pad0, pad + ch + k::px(8), w - 2 * pad0, b"No date to show until a time source answers.", k::TEXT_2(), false, ui);
        return;
    };
    // CLOCKPAINTERS (B719): the local civil fields and the week's first day are `clock::locale`'s (Principia's
    // `system.locale.tz` and `.first_day`); `DAYS` / `DAY_HEAD` stay Monday-indexed, so Sunday = 0 maps to 6.
    let loc = crate::clock::locale::current();
    let c = crate::clock::locale::civil(crate::clock::locale::Painter::Calendar, secs);
    let (y, mo, d) = (c.y, c.mo, c.d);
    let dow = (c.wd as usize + 6) % 7;
    let mut line = [0u8; 48];
    let mut n = 0;
    let mut push = |t: &[u8], line: &mut [u8; 48]| {
        let l = t.len().min(48 - n);
        line[n..n + l].copy_from_slice(&t[..l]);
        n += l;
    };
    let mut nb = [0u8; 4];
    push(DAYS[dow].as_bytes(), &mut line);
    push(b", ", &mut line);
    let l = num(d, &mut nb);
    push(&nb[..l], &mut line);
    push(b" ", &mut line);
    push(MONTHS[(mo as usize).clamp(1, 12) - 1].as_bytes(), &mut line);
    p::text(s, pad0, pad, w - 2 * pad0, &line[..n], k::TEXT(), true, ui);
    let mut yy = pad + ch + k::px(16);
    p::hairline_h(s, 0, yy, w, k::STROKE(), k::STROKE_A());
    yy += 1 + k::px(16);
    // The month title.
    let mut mt = [0u8; 24];
    let mn = MONTHS[(mo as usize).clamp(1, 12) - 1].as_bytes();
    mt[..mn.len()].copy_from_slice(mn);
    let yv = y.clamp(0, 9999) as u32;
    let ys = [b' ', b'0' + (yv / 1000 % 10) as u8, b'0' + (yv / 100 % 10) as u8, b'0' + (yv / 10 % 10) as u8, b'0' + (yv % 10) as u8];
    mt[mn.len()..mn.len() + 5].copy_from_slice(&ys);
    p::text(s, pad0, yy, w - 2 * pad0, &mt[..mn.len() + 5], k::TEXT(), true, ui);
    yy += ch + k::px(12);
    let cell = k::px(k::CAL_CELL);
    let gx = (w - 7 * cell) / 2;
    for i in 0..7 {
        let hd = DAY_HEAD[(loc.col_wd(i) as usize + 6) % 7];
        let tw = p::text_w(hd.as_bytes(), false, ui);
        p::text(s, gx + i * cell + (cell - tw) / 2, yy, cell, hd.as_bytes(), k::TEXT_2(), false, ui);
    }
    yy += ch + k::px(4);
    let first = loc.week_col(((c.wd as usize + 35 - (d as usize - 1) % 7) % 7) as u8);
    let nd = month_days(y, mo);
    for day in 1..=nd {
        let i = first + day as usize - 1;
        let (c, r) = (i % 7, i / 7);
        let (x, y0) = (gx + c * cell, yy + r * cell);
        let today = day == d;
        if today {
            let dd = cell - k::px(6);
            p::disc(s, x + (cell - dd) / 2, y0 + (cell - dd) / 2, dd, k::ACCENT(), 255);
        }
        let l = num(day, &mut nb);
        let tw = p::text_w(&nb[..l], today, ui);
        p::text(s, x + (cell - tw) / 2, y0 + (cell - ch) / 2, cell, &nb[..l], if today { k::ON_ACCENT } else { k::TEXT() }, today, ui);
    }
}

fn paint(st: &mut St) {
    st.epoch = crate::video::theme::epoch();
    match st.kind {
        2 => paint_calendar(st),
        3 => paint_quick(st),
        _ => paint_start(st),
    }
}

// ── SOFTY2 (B641): QUICK SETTINGS — the tray's flyout: Wi-Fi, sound, battery ─────────────────────────────────────
// The facts are `video::status`'s (the tray's own snapshot and the battery reading), the one write is the volume
// (`status::set_volume`, the Settings slider's own door). A row with no device says so and takes no press.

const T_QS_NET: u16 = 300;
const T_QS_MUTE: u16 = 301;
const T_QS_VOL: u16 = 302;
const T_QS_GEAR: u16 = 303;

struct QGeo {
    tile: p::Rect,
    mute: p::Rect,
    track: p::Rect,
    foot_y: usize,
    gear: p::Rect,
}

fn qs_height() -> usize {
    let pad = k::px(k::QS_PAD);
    pad + k::px(k::QS_TILE_H) + pad + k::px(k::QS_ROW) + pad + k::px(k::FOOT_H)
}

fn qgeo(w: usize, h: usize) -> QGeo {
    let pad = k::px(k::QS_PAD);
    let tile = (pad, pad, w - 2 * pad, k::px(k::QS_TILE_H));
    let ry = tile.1 + tile.3 + pad;
    let b = k::px(k::QS_BTN);
    let mute = (pad, ry + (k::px(k::QS_ROW) - b) / 2, b, b);
    let tx = mute.0 + b + k::px(16);
    let knob = k::px(k::QS_KNOB);
    let track = (tx, ry + (k::px(k::QS_ROW) - knob) / 2, (w - pad).saturating_sub(tx), knob);
    let foot_y = h.saturating_sub(k::px(k::FOOT_H));
    let g = k::px(k::POWER_BTN);
    let gear = (w - pad - g, foot_y + (k::px(k::FOOT_H) - g) / 2, g, g);
    QGeo { tile, mute, track, foot_y, gear }
}

fn qtarget(st: &St, x: usize, y: usize) -> u16 {
    let g = qgeo(st.w, st.h);
    if inside(g.tile, x, y) {
        T_QS_NET
    } else if inside(g.mute, x, y) {
        T_QS_MUTE
    } else if inside(g.track, x, y) {
        T_QS_VOL
    } else if inside(g.gear, x, y) {
        T_QS_GEAR
    } else {
        T_NONE
    }
}

/// The tray's facts, read once: `net` 0 none / 1 down / 2 up (+ the lease), the volume when a sink exists, the battery.
fn qfacts() -> (u8, Option<[u8; 4]>, Option<(u8, bool)>, Option<(u16, bool)>) {
    let (mut net, mut ip, mut vol) = (0u8, None, None);
    if let Some(tr) = crate::video::status::tray() {
        if let Some(n) = tr.net {
            net = if n.up || n.ip.is_some() { 2 } else { 1 };
            ip = n.ip;
        }
        vol = tr.volume;
    }
    let bat = crate::video::status::reading().map(|(b, _)| (b.percent.min(100), b.charging));
    (net, ip, vol, bat)
}

/// Decimal digits of `v` into `out`, returning the length.
fn dec(mut v: u32, out: &mut [u8]) -> usize {
    let mut d = [0u8; 10];
    let mut k = 0;
    while k == 0 || v > 0 {
        d[k] = b'0' + (v % 10) as u8;
        v /= 10;
        k += 1;
    }
    let n = k.min(out.len());
    for i in 0..n {
        out[i] = d[k - 1 - i];
    }
    n
}

fn paint_quick(st: &mut St) {
    let (w, h, hot, m) = (st.w, st.h, st.hot, st.m);
    let g = qgeo(w, h);
    let (mut s, ox, oy) = st.surf();
    let s = &mut s;
    body(s, w, h, Some(g.foot_y), (ox, oy), m);
    let ui = Face::Ui;
    let ch = ui.cell_h();
    let (net, ip, vol, bat) = qfacts();
    // Wi-Fi / network: one tile, lit in the accent while a link is up.
    let t = g.tile;
    let rad = k::px(k::QS_TILE_R);
    let up = net == 2;
    let (face, ink, ink2) = if up { (k::ACCENT(), k::ON_ACCENT, k::ON_ACCENT) } else { (k::FIELD(), k::TEXT(), k::TEXT_2()) };
    p::rrect(s, t, rad, face, 255);
    if !up {
        p::stroke_rrect(s, t, rad, k::STROKE(), 2 * k::STROKE_A());
    }
    if hot == T_QS_NET {
        p::rrect(s, t, rad, if up { k::PRESS() } else { k::HOVER() }, if up { 2 * k::PRESS_A() } else { k::HOVER_A() / 2 });
    }
    let ic = k::px(k::TRAY_ICON);
    let ix = t.0 + k::px(16);
    super::taskbar::glyph_net(s, ix, t.1 + (t.3 - ic) / 2, ic, net != 1, ink);
    let tx = ix + ic + k::px(14);
    let ty = t.1 + (t.3 - 2 * ch) / 2;
    p::text(s, tx, ty, t.0 + t.2 - tx - k::px(8), if net == 0 { b"Network" } else { b"Wi-Fi and network" }, ink, true, ui);
    let mut lb = [0u8; 40];
    let st_line: &[u8] = match (net, ip) {
        (0, _) => b"No network adapter",
        (1, _) => b"Not connected",
        (_, Some(a)) => {
            let head = b"Connected ";
            lb[..head.len()].copy_from_slice(head);
            let mut l = head.len();
            for (i, o) in a.iter().enumerate() {
                l += dec(*o as u32, &mut lb[l..]);
                if i < 3 {
                    lb[l] = b'.';
                    l += 1;
                }
            }
            &lb[..l]
        }
        _ => b"Connected",
    };
    p::text(s, tx, ty + ch, t.0 + t.2 - tx - k::px(8), st_line, ink2, false, ui);
    // Sound: the mute button and the level slider (the accent fill to the knob).
    let mb = g.mute;
    let tr = g.track;
    match vol {
        Some((lv, muted)) => {
            if hot == T_QS_MUTE || muted {
                p::rrect(s, mb, k::px(k::TILE_R), k::HOVER(), if muted { k::HOVER_A() } else { k::HOVER_A() / 2 });
                p::stroke_rrect(s, mb, k::px(k::TILE_R), k::STROKE(), k::STROKE_A());
            }
            super::taskbar::glyph_vol(s, mb.0 + (mb.2 - ic) / 2, mb.1 + (mb.3 - ic) / 2, ic, lv, muted, k::TEXT());
            let th = k::px(k::QS_TRACK_H);
            let knob = tr.3;
            let span = tr.2.saturating_sub(knob);
            let kx = tr.0 + span * lv.min(16) as usize / 16;
            let cy = tr.1 + (knob - th) / 2;
            p::rrect(s, (tr.0, cy, tr.2, th), th / 2, k::FIELD_LINE(), 255);
            let fill = if muted { k::TEXT_IDLE() } else { k::ACCENT() };
            p::rrect(s, (tr.0, cy, kx + knob / 2 - tr.0, th), th / 2, fill, 255);
            p::disc(s, kx, tr.1, knob, k::FIELD(), 255);
            p::stroke_rrect(s, (kx, tr.1, knob, knob), knob / 2, k::STROKE(), 3 * k::STROKE_A());
            p::disc(s, kx + knob / 4, tr.1 + knob / 4, knob - 2 * (knob / 4), if hot == T_QS_VOL { k::ACCENT_PRESSED() } else { fill }, 255);
        }
        None => {
            super::taskbar::glyph_vol(s, mb.0 + (mb.2 - ic) / 2, mb.1 + (mb.3 - ic) / 2, ic, 0, true, k::TEXT_IDLE());
            p::text(s, tr.0, tr.1 + (tr.3 - ch) / 2, tr.2, b"No sound output", k::TEXT_2(), false, ui);
        }
    }
    // The footer: the battery, and the gear that opens Settings.
    let fy = g.foot_y;
    let pad = k::px(k::QS_PAD);
    let mut bl = [0u8; 24];
    let blen = match bat {
        Some((pc, chg)) => {
            super::taskbar::glyph_bat(s, pad, fy + (k::px(k::FOOT_H) - ic) / 2, ic, pc, chg, k::TEXT());
            let mut l = dec(pc as u32, &mut bl);
            let tail: &[u8] = if chg { b" %  charging" } else { b" %" };
            bl[l..l + tail.len()].copy_from_slice(tail);
            l += tail.len();
            l
        }
        None => {
            let t: &[u8] = b"No battery";
            bl[..t.len()].copy_from_slice(t);
            t.len()
        }
    };
    let bx = pad + if bat.is_some() { ic + k::px(10) } else { 0 };
    p::text(s, bx, fy + (k::px(k::FOOT_H) - ch) / 2, w / 2, &bl[..blen], k::TEXT(), false, ui);
    let gr = g.gear;
    if hot == T_QS_GEAR {
        p::rrect(s, gr, k::px(k::TILE_R), k::HOVER(), k::HOVER_A());
        p::stroke_rrect(s, gr, k::px(k::TILE_R), k::STROKE(), k::STROKE_A());
    }
    // Our gear: a ring and six short spokes.
    let gd = k::px(14);
    let (gcx, gcy) = (gr.0 + gr.2 / 2, gr.1 + gr.3 / 2);
    p::stroke_rrect(s, (gcx - gd / 2, gcy - gd / 2, gd, gd), gd / 2, k::TEXT(), 255);
    p::stroke_rrect(s, (gcx - gd / 5, gcy - gd / 5, 2 * (gd / 5), 2 * (gd / 5)), gd / 5, k::TEXT(), 255);
    const SPOKE: [(i32, i32); 6] = [(0, -8), (7, -4), (7, 4), (0, 8), (-7, 4), (-7, -4)];
    for (dx, dy) in SPOKE {
        let (r0, r1) = (gd as i32 / 2, gd as i32 / 2 + k::px(3) as i32);
        let ex = |r: i32| (gcx as i32 + dx * r / 8).max(0) as usize;
        let ey = |r: i32| (gcy as i32 + dy * r / 8).max(0) as usize;
        p::line(s, ex(r0), ey(r0), ex(r1), ey(r1), k::px(2), k::TEXT(), 255);
    }
}

/// A press on quick settings' target `t` at body-local `x` (the slider reads it).
fn qact(t: u16, x: usize) {
    let (_, _, vol, _) = qfacts();
    match t {
        T_QS_MUTE => {
            if let Some((lv, muted)) = vol {
                let amp = crate::video::status::set_volume(lv, !muted);
                serial_println!("[softy] quick volume level={}/16 muted={} amp_written={} by=mute", lv, !muted as u8, amp as u8);
            }
        }
        T_QS_VOL => {
            if vol.is_some() {
                let (w, h) = ST.lock().as_ref().map(|s| (s.w, s.h)).unwrap_or((0, 0));
                let tr = qgeo(w, h).track;
                let span = tr.2.saturating_sub(tr.3).max(1);
                let lv = ((x.saturating_sub(tr.0 + tr.3 / 2)).min(span) * 16 + span / 2) / span;
                let amp = crate::video::status::set_volume(lv as u8, false);
                serial_println!("[softy] quick volume level={}/16 muted=0 amp_written={} by=slider", lv, amp as u8);
            }
        }
        T_QS_NET | T_QS_GEAR => {
            close("pick");
            let how = dock::launch_named("settings").unwrap_or("absent");
            serial_println!("[softy] quick {} -> settings launch {}", if t == T_QS_NET { "network" } else { "gear" }, how);
            return;
        }
        _ => return,
    }
    let id = {
        let mut g = ST.lock();
        let Some(st) = g.as_mut() else { return };
        paint(st);
        st.win
    };
    let _ = wm::present(id);
}

// ── open / close ──────────────────────────────────────────────────────────────────────────────────────────────

fn panel() -> Option<(usize, usize)> {
    let fb = *crate::video::WRITER.lock();
    if !fb.is_ready() {
        return None;
    }
    Some((fb.width(), fb.height()))
}

/// Open flyout `kind` (1 Start, 2 the calendar, 3 quick settings); a different one open is closed first. `false` on
/// a refusal.
pub fn open(kind: u8, by: &str) -> bool {
    if open_kind() == kind {
        return true;
    }
    close(by);
    let Some((pw, ph)) = panel() else { return false };
    let top = super::taskbar::top(pw, ph);
    let gap = k::px(k::FLY_GAP);
    let (w, h, x) = match kind {
        2 => {
            let w = k::px(k::CAL_W).min(pw.saturating_sub(2 * gap));
            (w, cal_height().min(top.saturating_sub(2 * gap)), pw.saturating_sub(k::px(k::EDGE_PAD) + w))
        }
        3 => {
            let w = k::px(k::QS_W).min(pw.saturating_sub(2 * gap));
            (w, qs_height().min(top.saturating_sub(2 * gap)), pw.saturating_sub(k::px(k::EDGE_PAD) + w))
        }
        _ => {
            let w = k::px(k::START_W).min(pw.saturating_sub(2 * gap));
            (w, k::px(k::START_H).min(top.saturating_sub(2 * gap)), pw.saturating_sub(w) / 2)
        }
    };
    if w < k::px(160) || h < k::px(160) {
        serial_println!("[softy] {} open REFUSED reason=panel {}x{} by={}", word(kind), pw, ph, by);
        return false;
    }
    let y = top.saturating_sub(gap + h);
    // SOFTY2: the shadow band — `FLY_GAP` wide, held inside the panel and above the taskbar.
    let m = gap.min(x).min(y).min(pw.saturating_sub(x + w)).min(top.saturating_sub(y + h));
    let (sw, sh) = (w + 2 * m, h + 2 * m);
    let mut buf: Vec<u32> = Vec::new();
    if buf.try_reserve_exact(sw * sh).is_err() {
        serial_println!("[softy] {} open REFUSED reason=alloc by={}", word(kind), by);
        return false;
    }
    buf.resize(sw * sh, k::FLYOUT());
    let mut st = St { kind, win: wm::WIN_NONE, x, y, w, h, m, hot: T_NONE, power: false, minute: 0, epoch: 0, buf };
    paint(&mut st);
    let (addr, len) = (st.buf.as_ptr() as usize, st.buf.len() * 4);
    *ST.lock() = Some(st);
    let (wx, wy) = (x - m, y - m);
    // OVERLAYENTRY-ARM (B847): the one overlay door on both desktop families — minted, placed at (wx, wy) and presented, one composite.
    let id = wm::overlay_open(addr, len, sw, sh, wx, wy);
    if id == wm::WIN_NONE {
        *ST.lock() = None;
        serial_println!("[softy] {} open REFUSED reason=overlay by={}", word(kind), by);
        return false;
    }
    if let Some(st) = ST.lock().as_mut() {
        st.win = id;
    }
    KIND.store(kind, Ordering::Release);
    // SOFTY2: `shadow=` is the band's width (0 = no room for the elev.2 shadow); the calendar names notify's unread count.
    if kind == 2 {
        serial_println!("[softy] {} open by={} win={} {}x{} at={},{} shadow={} notify_unread={}", word(kind), by, id, w, h, x, y, m, crate::video::notify::unread());
        crate::video::notify::request_center(true); // SOFTY3 (B651): the calendar opens notify's OWN Center beside it (its service pass acts)
    } else {
        serial_println!("[softy] {} open by={} win={} {}x{} at={},{} shadow={}", word(kind), by, id, w, h, x, y, m);
    }
    true
}

/// Close whatever flyout is open (idempotent).
pub fn close(by: &str) -> bool {
    let kind = KIND.swap(0, Ordering::AcqRel);
    if kind == 0 {
        return false;
    }
    let id = ST.lock().as_ref().map(|s| s.win).unwrap_or(wm::WIN_NONE);
    if id != wm::WIN_NONE {
        wm::close(id);
    }
    *ST.lock() = None; // after the row is gone
    serial_println!("[softy] {} close by={}", word(kind), by);
    if kind == 2 {
        crate::video::notify::request_center(false); // SOFTY3 (B651): the calendar's Center closes with it
    }
    true
}

/// Toggle flyout `kind`.
pub fn toggle(kind: u8, by: &str) -> bool {
    if open_kind() == kind {
        close(by);
        false
    } else {
        open(kind, by)
    }
}

/// SOFTY2: the open flyout's shadow band (px; 0 = none open or no room for one).
pub fn shadow_band() -> usize {
    ST.lock().as_ref().map(|s| s.m).unwrap_or(0)
}

/// The flyout's panel rect while one is open.
pub fn rect() -> Option<(usize, usize, usize, usize)> {
    ST.lock().as_ref().map(|s| (s.x, s.y, s.w, s.h))
}

/// A press INSIDE the open flyout is its own (`true`); anything else is the caller's (it closes the flyout).
pub fn press_inside(x: i32, y: i32) -> bool {
    if open_kind() == 0 || x < 0 || y < 0 {
        return false;
    }
    let (px, py) = (x as usize, y as usize);
    let (t, kind) = {
        let mut g = ST.lock();
        let Some(st) = g.as_mut() else { return false };
        if !inside((st.x, st.y, st.w, st.h), px, py) {
            return false;
        }
        if st.kind == 2 {
            return true;
        }
        let (lx, ly) = (px - st.x, py - st.y);
        if st.kind == 3 {
            (qtarget(st, lx, ly), lx as u16 as u32 | 3 << 16)
        } else {
            (target(st, lx, ly), 1 << 16)
        }
    };
    if kind >> 16 == 3 {
        qact(t, (kind & 0xFFFF) as usize);
    } else {
        act(t);
    }
    true
}

fn act(t: u16) {
    match t {
        T_SEARCH => {
            close("search");
            #[cfg(all(target_arch = "x86_64", feature = "wc"))]
            {
                let ok = crate::video::launcher::open();
                serial_println!("[softy] start search -> launcher open={}", ok as u8);
            }
        }
        T_POWER => {
            let id = {
                let mut g = ST.lock();
                let Some(st) = g.as_mut() else { return };
                st.power = !st.power;
                paint(st);
                st.win
            };
            let _ = wm::present(id);
        }
        _ if t >= T_POWER_ROW && t < T_POWER_ROW + POWER_ROWS.len() as u16 => {
            close("pick");
            let r = (t - T_POWER_ROW) as u8;
            serial_println!("[softy] start power row={}", POWER_ROWS[r as usize]);
            match r {
                0 => crate::video::crystal::power_fire(4),
                1 => {
                    let _ = crate::video::dialog::power_confirm(1);
                }
                _ => {
                    let _ = crate::video::dialog::power_confirm(2);
                }
            }
        }
        _ if t >= T_GRID && t < T_POWER => {
            let mut items = [0u8; 16];
            let n = grid_items(&mut items);
            let i = (t - T_GRID) as usize;
            if i < n {
                close("pick");
                let name = dock::DP_PINS[items[i] as usize].name;
                let how = dock::launch_named(name).unwrap_or("absent");
                serial_println!("[softy] start launch app={} -> {}", name, how);
            }
        }
        _ => {
            // The menu's own background: consumed; an open power menu folds away.
            let id = {
                let mut g = ST.lock();
                let Some(st) = g.as_mut() else { return };
                if !st.power {
                    return;
                }
                st.power = false;
                paint(st);
                st.win
            };
            let _ = wm::present(id);
        }
    }
}

/// **The key door** (asked from x86's `wc_route_event` door chain since WINKEY, B665): Esc closes an open flyout. `true` =
/// consumed. ESC ONLY — it never toggles Start (the Windows-key tap reaches Start through [`key_service`] alone).
pub fn key_door(ev: crate::pal::Event) -> bool {
    if open_kind() == 0 {
        return false;
    }
    if let crate::pal::Event::Key(0x1B) = ev {
        close("key");
        return true;
    }
    false
}

/// The Win/Command key's verb: toggle the Start menu (`by=key`).
pub fn toggle_by_key() -> bool {
    toggle(1, "key")
}

/// The look's service pass: the Windows key's taps and Esc (SOFTY2), the hover under the pointer, the calendar's
/// minute and notify count, quick settings' facts, and a repaint after an appearance change. Off the input router.
pub fn service() {
    key_service();
    if open_kind() == 0 {
        return;
    }
    let Some((pw, ph)) = panel() else { return };
    let (cx, cy) = crate::pal::cursor::pos(pw as i32, ph as i32);
    let minute = crate::clock::try_unix_now().map(|s| s / 60).unwrap_or(0);
    let epoch = crate::video::theme::epoch();
    let id = {
        let Some(mut g) = ST.try_lock() else { return };
        let Some(st) = g.as_mut() else { return };
        let over = cx >= 0 && cy >= 0 && inside((st.x, st.y, st.w, st.h), cx as usize, cy as usize);
        let (lx, ly) = (cx.max(0) as usize, cy.max(0) as usize);
        let hot = match st.kind {
            1 if over => target(st, lx - st.x, ly - st.y),
            3 if over => qtarget(st, lx - st.x, ly - st.y),
            _ => T_NONE,
        };
        // What the open flyout shows that can move under it: the minute and notify's count (the calendar), the
        // tray's facts (quick settings).
        let sig = match st.kind {
            2 => minute ^ (crate::video::notify::unread() as u64) << 40 ^ (crate::video::notify::dnd() as u64) << 63,
            3 => {
                let (n, ip, v, b) = qfacts();
                let ipw = ip.map(|a| u32::from_be_bytes(a) as u64).unwrap_or(0);
                n as u64 ^ ipw << 8 ^ v.map(|(l, mu)| 1 + l as u64 + ((mu as u64) << 6)).unwrap_or(0) << 40 ^ b.map(|(p, c)| 1 + p as u64 + ((c as u64) << 8)).unwrap_or(0) << 48
            }
            _ => 0,
        };
        if hot == st.hot && sig == st.minute && epoch == st.epoch {
            return;
        }
        st.hot = hot;
        st.minute = sig;
        paint(st);
        st.win
    };
    let _ = wm::present(id);
}

/// SOFTY2 — THE WINDOWS KEY'S DOOR, off the input router: the taps `keymap` counted (a bare press-and-release of
/// the key that plays the Windows role on this keyboard — Option on the Mac, the Win key on a PC; R126) toggle the
/// Start menu, an Esc edge closes an open flyout, and a change of the live binding table is named once.
fn key_service() {
    let taps = crate::video::keymap::take_start_taps();
    if taps != 0 && crate::video::look::is_softy() {
        let role = crate::video::keymap::start_role_name();
        if taps % 2 == 1 {
            let on = toggle(1, "key");
            serial_println!("[softy] start key role={} taps={} -> {}", role, taps, if on { "open" } else { "close" });
        } else {
            serial_println!("[softy] start key role={} taps={} -> unchanged", role, taps);
        }
    }
    if crate::video::keymap::take_esc_edges() != 0 && open_kind() != 0 {
        close("key");
    }
    // The table follows the look and the keyboard (`keymap::active`): name it when it moves, never at boot.
    static SEEN: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    let t = crate::video::keymap::active();
    let p = t as *const crate::video::keymap::Table as usize;
    let was = SEEN.swap(p, Ordering::Relaxed);
    if was != 0 && was != p {
        crate::video::keymap::announce("look");
    }
}

/// WINKEY (rmbp-ledger B665) — `tests winkey`, on the running desktop with Softy on the glass; restores the look it found.
/// Start's ONE trigger is the tap counter (`keymap::tap_track` → [`key_service`]); the door ([`key_door`]) is Esc only.
///
/// * `tap=` — one bare press-and-release of the Windows-role key (`keymap::winkey_drive`), then one service pass: Start
///   is open (`open`), exactly once.
/// * `tap2=` — a second tap and pass: closed.
/// * `chord=` — the key down, an `e` edge, the key up (Win+E): the pass leaves Start closed (`ignored`).
/// * `esc=` — a tap opens Start; the door refuses `Action::Start` (it never toggles) and takes Esc (`Key(0x1B)`), which
///   closes it (`closed`).
/// * `una=` — under the Una look the same tap counts nothing and the pass opens nothing (`ignored`).
/// * `action_code=` — `clipboard::action_code(Action::Start)`, the ring-3 `INPUT_EV_ACTION` payload (50).
///
/// `:: WINKEY: tap=open tap2=closed chord=ignored esc=closed una=ignored action_code=50 -> PASS ::`.
#[cfg(feature = "witness")]
pub fn winkey_selftest() {
    use crate::video::{keymap, look};
    if panel().is_none() {
        serial_println!(":: WINKEY: -> SKIP :: reason=no-panel");
        return;
    }
    #[cfg(feature = "login")]
    if !crate::video::desktopbuild::built() {
        serial_println!(":: WINKEY: -> SKIP :: reason=no-desktop (the furniture is built at login)");
        return;
    }
    let was = look::current();
    let _ = close("tests");
    let _ = look::set(look::Look::Softy, "tests");
    let role = keymap::start_role_name();
    let _ = keymap::take_start_taps();
    let _ = keymap::take_esc_edges();
    let tap = |chord: bool| {
        keymap::winkey_drive(chord);
        key_service();
        open_kind()
    };
    let tap_ok = tap(false) == 1;
    let tap2_ok = tap(false) == 0;
    let chord_ok = tap(true) == 0;
    let esc_open = tap(false) == 1;
    let door_no_toggle = !key_door(crate::pal::Event::Action(keymap::Action::Start)) && open_kind() == 1;
    let esc_ok = esc_open && door_no_toggle && key_door(crate::pal::Event::Key(0x1B)) && open_kind() == 0;
    let _ = close("tests");
    let _ = look::set(look::Look::Una, "tests");
    let una_ok = tap(false) == 0;
    let _ = close("tests");
    let _ = look::set(was, "tests");
    let _ = keymap::take_start_taps();
    let _ = keymap::take_esc_edges();
    let code = crate::video::clipboard::action_code(keymap::Action::Start);
    let pass = tap_ok && tap2_ok && chord_ok && esc_ok && una_ok && code == 50;
    serial_println!(
        ":: WINKEY: tap={} tap2={} chord={} esc={} una={} action_code={} -> {} :: role={}",
        if tap_ok { "open" } else { "no" },
        if tap2_ok { "closed" } else { "no" },
        if chord_ok { "ignored" } else { "opened" },
        if esc_ok { "closed" } else { "no" },
        if una_ok { "ignored" } else { "opened" },
        code,
        if pass { "PASS" } else { "FAIL" },
        role
    );
}

// ── GLYPHS (rmbp-ledger B676) — the Start menu's fixed-box labels, for `tests glyphs`' pseudo-locale pass ─────────

/// GLYPHS (B676): the Start menu's labels whose box does not depend on the flyout's size, as `(surface, label, box
/// px, bold, face)` — the power menu's rows (drawn at `m.0 + px(16)` clipped to `POWER_W - px(24)`) and the grid's
/// app names (centred in the `CELL_W` column, clipped to `CELL_W - px(8)`), exactly the clips [`paint_start`]'s
/// `p::text` calls pass. Measurement only — no layout reads this.
pub fn label_boxes(out: &mut Vec<(&'static str, alloc::string::String, usize, bool, Face)>) {
    let row_box = k::px(k::POWER_W).saturating_sub(k::px(24));
    for l in POWER_ROWS {
        out.push(("softy-power", alloc::string::String::from(l), row_box, false, Face::Ui));
    }
    let cell_box = k::px(k::CELL_W).saturating_sub(k::px(8));
    let mut items = [0u8; 16];
    let n = grid_items(&mut items);
    for &pin in &items[..n] {
        let mut lb = [0u8; 16];
        let ll = label(pin as usize, &mut lb);
        out.push(("softy-start", alloc::string::String::from_utf8_lossy(&lb[..ll]).into_owned(), cell_box, false, Face::Ui));
    }
}
