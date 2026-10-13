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
//! THE CALENDAR — over the clock: the long date, the month and its grid with today in the accent.
//!
//! Opened by the Start button / the clock (`by=button`), by a key door (`by=key`, the Win/Command door owed in the
//! x86 router — SOFTY.md), closed by a press outside (`by=outside`), Esc (`by=key`), a pick (`by=pick`) or a look
//! switch (`by=switch`). Hover repaints ride the look's service pass ([`service`]), never the input router.
//!
//! Wire: `[softy] start open|close by=<button|key|outside|pick|switch|search>` (the calendar: `[softy] calendar …`).

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU8, Ordering};

use super::paint::{self as p, Surf};
use super::palette as k;
use crate::video::{dock, text::Face, wm};

/// 0 closed, 1 the Start menu, 2 the calendar.
static KIND: AtomicU8 = AtomicU8::new(0);

struct St {
    kind: u8,
    win: wm::WinId,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    hot: u16,
    power: bool,
    minute: u64,
    buf: Vec<u32>,
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
    if kind == 2 { "calendar" } else { "start" }
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

fn body(s: &mut Surf, w: usize, h: usize, foot: Option<usize>) {
    let r = k::px(k::FLY_R);
    p::fill(s, (0, 0, w, h), k::DESKTOP, 255);
    match foot {
        Some(fy) => {
            p::rrect(s, (0, 0, w, h), r, k::FLYOUT_FOOT, 255);
            p::rrect_top(s, (0, 0, w, fy), r, k::FLYOUT, 255);
            p::hairline_h(s, 0, fy, w, k::STROKE, k::STROKE_A);
        }
        None => p::rrect(s, (0, 0, w, h), r, k::FLYOUT, 255),
    }
    p::stroke_rrect(s, (0, 0, w, h), r, k::STROKE, k::STROKE_A);
}

fn paint_start(st: &mut St) {
    let (w, h, hot, power) = (st.w, st.h, st.hot, st.power);
    let g = geo(w, h);
    let mut s = Surf::whole(&mut st.buf, w, h);
    let s = &mut s;
    body(s, w, h, Some(g.foot_y));
    let ui = Face::Ui;
    let ch = ui.cell_h();
    // The search field: white, a hairline, the darker underline that says "type here", a magnifier, a placeholder.
    let sr = g.search;
    let rad = sr.3 / 2;
    p::rrect(s, sr, rad, k::FIELD, 255);
    p::stroke_rrect(s, sr, rad, k::STROKE, if hot == T_SEARCH { 2 * k::STROKE_A } else { k::STROKE_A });
    p::hairline_h(s, sr.0 + rad, sr.1 + sr.3 - 1, sr.2 - 2 * rad, k::FIELD_LINE, 255);
    let gl = k::px(12);
    let (gx, gy) = (sr.0 + rad, sr.1 + (sr.3 - gl) / 2);
    p::stroke_rrect(s, (gx, gy, gl * 3 / 4, gl * 3 / 4), gl * 3 / 8, k::TEXT_2, 255);
    p::line(s, gx + gl * 5 / 8, gy + gl * 5 / 8, gx + gl, gy + gl, k::px(1), k::TEXT_2, 255);
    let tx = gx + gl + k::px(10);
    p::text(s, tx, sr.1 + (sr.3 - ch) / 2, sr.0 + sr.2 - rad - tx, b"Search apps, settings and files", k::TEXT_2, false, ui);
    // "Pinned".
    p::text(s, g.pad + k::px(8), g.head_y, w / 2, b"Pinned", k::TEXT, true, ui);
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
            p::rrect(s, cell, k::px(k::CELL_R), k::HOVER, 255);
            p::stroke_rrect(s, cell, k::px(k::CELL_R), k::STROKE, k::STROKE_A);
        }
        let pin = items[i] as usize;
        let (ix, iy) = (x + (g.cw - isz) / 2, y + k::px(14));
        super::taskbar::tile_icon(s, pin as u8, b"", ix, iy, isz);
        let mut lb = [0u8; 16];
        let ll = label(pin, &mut lb);
        let lw = p::text_w(&lb[..ll], false, ui).min(g.cw - k::px(8));
        p::text(s, x + (g.cw - lw) / 2, iy + isz + k::px(8), g.cw - k::px(8), &lb[..ll], k::TEXT, false, ui);
    }
    // The footer: the user, the power button.
    let av = k::px(k::AVATAR);
    let (ax, ay) = (g.pad + k::px(8), g.foot_y + (k::px(k::FOOT_H) - av) / 2);
    p::disc(s, ax, ay, av, k::ACCENT, 255);
    let mut wb = [0u8; crate::prefs::WHO_BUF];
    let who = crate::prefs::user_name_in(&mut wb).map(|n| n.as_bytes()).unwrap_or(&b"UnaOS"[..]);
    let who: &[u8] = if who.is_empty() { b"UnaOS" } else { who };
    let ini = [who[0].to_ascii_uppercase()];
    let iw = p::text_w(&ini, true, ui);
    p::text(s, ax + (av - iw) / 2, ay + (av - ch) / 2, av, &ini, k::ON_ACCENT, true, ui);
    p::text(s, ax + av + k::px(12), g.foot_y + (k::px(k::FOOT_H) - ch) / 2, w / 2, who, k::TEXT, false, ui);
    let pr = g.power;
    if hot == T_POWER || power {
        p::rrect(s, pr, k::px(k::TILE_R), k::HOVER, 255);
        p::stroke_rrect(s, pr, k::px(k::TILE_R), k::STROKE, k::STROKE_A);
    }
    // Our power glyph: a ring open at the top, its stem through the gap.
    let gd = k::px(16);
    let (px0, py0) = (pr.0 + (pr.2 - gd) / 2, pr.1 + (pr.3 - gd) / 2);
    p::stroke_rrect(s, (px0, py0, gd, gd), gd / 2, k::TEXT, 255);
    p::stroke_rrect(s, (px0 + 1, py0 + 1, gd - 2, gd - 2), (gd - 2) / 2, k::TEXT, 140);
    let back = if hot == T_POWER || power { p::blend(k::FLYOUT_FOOT, k::HOVER, 255) } else { k::FLYOUT_FOOT };
    p::fill(s, (px0 + gd / 2 - gd / 5, py0, 2 * (gd / 5), gd / 3), back, 255);
    p::line(s, px0 + gd / 2, py0 - k::px(1), px0 + gd / 2, py0 + gd / 2, k::px(1), k::TEXT, 255);
    // The power menu.
    if power {
        let m = g.menu;
        p::rrect(s, m, k::px(k::FLY_R), k::FIELD, 255);
        p::stroke_rrect(s, m, k::px(k::FLY_R), k::STROKE, 2 * k::STROKE_A);
        let rh = k::px(k::POWER_ROW_H);
        for (i, lbl) in POWER_ROWS.iter().enumerate() {
            let ry = m.1 + k::px(4) + i * rh;
            if hot == T_POWER_ROW + i as u16 {
                p::rrect(s, (m.0 + k::px(4), ry, m.2 - k::px(8), rh), k::px(4), k::PRESS, k::PRESS_A * 2);
            }
            p::text(s, m.0 + k::px(16), ry + (rh - ch) / 2, m.2 - k::px(24), lbl.as_bytes(), k::TEXT, false, ui);
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
    2 * k::px(k::CAL_PAD) + ch + k::px(16) + 1 + k::px(16) + ch + k::px(12) + ch + k::px(4) + 6 * k::px(k::CAL_CELL)
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
    let (w, h) = (st.w, st.h);
    let mut s = Surf::whole(&mut st.buf, w, h);
    let s = &mut s;
    body(s, w, h, None);
    let ui = Face::Ui;
    let ch = ui.cell_h();
    let pad = k::px(k::CAL_PAD);
    let Some(secs) = crate::clock::try_unix_now() else {
        p::text(s, pad, pad, w - 2 * pad, b"The clock is not set", k::TEXT, true, ui);
        p::text(s, pad, pad + ch + k::px(8), w - 2 * pad, b"No date to show until a time source answers.", k::TEXT_2, false, ui);
        return;
    };
    let (y, mo, d, _, _, _) = crate::clock::civil_from_unix(secs);
    let days = secs / 86_400;
    let dow = ((days + 3) % 7) as usize; // Monday-first; 1970-01-01 was a Thursday
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
    p::text(s, pad, pad, w - 2 * pad, &line[..n], k::TEXT, true, ui);
    let mut yy = pad + ch + k::px(16);
    p::hairline_h(s, 0, yy, w, k::STROKE, k::STROKE_A);
    yy += 1 + k::px(16);
    // The month title.
    let mut mt = [0u8; 24];
    let mn = MONTHS[(mo as usize).clamp(1, 12) - 1].as_bytes();
    mt[..mn.len()].copy_from_slice(mn);
    let yv = y.clamp(0, 9999) as u32;
    let ys = [b' ', b'0' + (yv / 1000 % 10) as u8, b'0' + (yv / 100 % 10) as u8, b'0' + (yv / 10 % 10) as u8, b'0' + (yv % 10) as u8];
    mt[mn.len()..mn.len() + 5].copy_from_slice(&ys);
    p::text(s, pad, yy, w - 2 * pad, &mt[..mn.len() + 5], k::TEXT, true, ui);
    yy += ch + k::px(12);
    let cell = k::px(k::CAL_CELL);
    let gx = (w - 7 * cell) / 2;
    for (i, hd) in DAY_HEAD.iter().enumerate() {
        let tw = p::text_w(hd.as_bytes(), false, ui);
        p::text(s, gx + i * cell + (cell - tw) / 2, yy, cell, hd.as_bytes(), k::TEXT_2, false, ui);
    }
    yy += ch + k::px(4);
    let first = (dow + 35 - ((d as usize - 1) % 7)) % 7;
    let nd = month_days(y, mo);
    for day in 1..=nd {
        let i = first + day as usize - 1;
        let (c, r) = (i % 7, i / 7);
        let (x, y0) = (gx + c * cell, yy + r * cell);
        let today = day == d;
        if today {
            let dd = cell - k::px(6);
            p::disc(s, x + (cell - dd) / 2, y0 + (cell - dd) / 2, dd, k::ACCENT, 255);
        }
        let l = num(day, &mut nb);
        let tw = p::text_w(&nb[..l], today, ui);
        p::text(s, x + (cell - tw) / 2, y0 + (cell - ch) / 2, cell, &nb[..l], if today { k::ON_ACCENT } else { k::TEXT }, today, ui);
    }
}

fn paint(st: &mut St) {
    if st.kind == 2 { paint_calendar(st) } else { paint_start(st) }
}

// ── open / close ──────────────────────────────────────────────────────────────────────────────────────────────

fn panel() -> Option<(usize, usize)> {
    let fb = *crate::video::WRITER.lock();
    if !fb.is_ready() {
        return None;
    }
    Some((fb.width(), fb.height()))
}

/// Open flyout `kind` (1 Start, 2 the calendar); a different one open is closed first. `false` on a refusal.
pub fn open(kind: u8, by: &str) -> bool {
    if open_kind() == kind {
        return true;
    }
    close(by);
    let Some((pw, ph)) = panel() else { return false };
    let top = super::taskbar::top(pw, ph);
    let gap = k::px(k::FLY_GAP);
    let (w, h, x) = if kind == 2 {
        let w = k::px(k::CAL_W).min(pw.saturating_sub(2 * gap));
        (w, cal_height().min(top.saturating_sub(2 * gap)), pw.saturating_sub(k::px(k::EDGE_PAD) + w))
    } else {
        let w = k::px(k::START_W).min(pw.saturating_sub(2 * gap));
        (w, k::px(k::START_H).min(top.saturating_sub(2 * gap)), pw.saturating_sub(w) / 2)
    };
    if w < k::px(160) || h < k::px(160) {
        serial_println!("[softy] {} open REFUSED reason=panel {}x{} by={}", word(kind), pw, ph, by);
        return false;
    }
    let y = top.saturating_sub(gap + h);
    let mut buf: Vec<u32> = Vec::new();
    if buf.try_reserve_exact(w * h).is_err() {
        serial_println!("[softy] {} open REFUSED reason=alloc by={}", word(kind), by);
        return false;
    }
    buf.resize(w * h, k::FLYOUT);
    let mut st = St { kind, win: wm::WIN_NONE, x, y, w, h, hot: T_NONE, power: false, minute: 0, buf };
    paint(&mut st);
    let (addr, len) = (st.buf.as_ptr() as usize, st.buf.len() * 4);
    *ST.lock() = Some(st);
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    let id = wm::overlay_open(addr, len, w, h, x, y);
    // The Pi's desktop has no `overlay_open` (x86's Launcher door): the same chromeless row, placed by `move_to`.
    #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
    let id = {
        let id = wm::splash_open(addr, len, w, h);
        if id != wm::WIN_NONE {
            let _ = wm::move_to(id, x, y);
        }
        id
    };
    if id == wm::WIN_NONE {
        *ST.lock() = None;
        serial_println!("[softy] {} open REFUSED reason=overlay by={}", word(kind), by);
        return false;
    }
    if let Some(st) = ST.lock().as_mut() {
        st.win = id;
    }
    KIND.store(kind, Ordering::Release);
    serial_println!("[softy] {} open by={} win={} {}x{} at={},{}", word(kind), by, id, w, h, x, y);
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
        (target(st, px - st.x, py - st.y), st.kind)
    };
    let _ = kind;
    act(t);
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

/// **The key door** (owed in x86's `wc_route_event`, SOFTY.md): Esc closes an open flyout. `true` = consumed.
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

/// The look's service pass: the hover under the pointer, and the calendar's minute. Off the input router.
pub fn service() {
    if open_kind() == 0 {
        return;
    }
    let Some((pw, ph)) = panel() else { return };
    let (cx, cy) = crate::pal::cursor::pos(pw as i32, ph as i32);
    let minute = crate::clock::try_unix_now().map(|s| s / 60).unwrap_or(0);
    let id = {
        let Some(mut g) = ST.try_lock() else { return };
        let Some(st) = g.as_mut() else { return };
        let over = cx >= 0 && cy >= 0 && inside((st.x, st.y, st.w, st.h), cx as usize, cy as usize);
        let hot = if over && st.kind == 1 { target(st, cx as usize - st.x, cy as usize - st.y) } else { T_NONE };
        let stale = st.kind == 2 && st.minute != minute;
        if hot == st.hot && !stale {
            return;
        }
        st.hot = hot;
        st.minute = minute;
        paint(st);
        st.win
    };
    let _ = wm::present(id);
}
