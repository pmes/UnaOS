// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! CHARTER: Kernel — wm
//!
//! STATUSTRAY (rmbp-ledger B426, MACPARITY row 3) — **the status items' MENUS.** Each item on the bar's right
//! end (input, network, volume, battery, clock) opens a drop-down on press; a press on another item switches
//! to its menu (winmenu's sticky rule for titles); a press anywhere else, or Escape, closes it.
//!
//! # One drop-down, not a second
//!
//! The drop-down is the SHARD dropdown's PANEL MODE (`crystal.rs`, POWERMENU M2) — the surface the battery
//! item already opened. This module decides WHICH item is open, builds that item's rows from the MODEL
//! (`status::tray`, `status::reading`, the civil clock), anchors the panel under the item, and acts on a
//! row press through the owner's existing seam (`status::set_volume` — Settings' and the keys' writer;
//! `keymap::set_pc` — the table selection). It owns no store of its own beyond "which item is open".
//!
//! Two row kinds are painted by this module inside the panel (the panel's text lines carry a marker
//! byte): a SLIDER (`\x01NN`, the volume level of 16, the press position sets it — BRIGHTSLIDER's click
//! seam; the drag is SLIDERDRAG's, owed) and a GRID row (`\x02c0|c1|…|c6`, the clock's seven-column month
//! glance; a cell starting `*` is today).
//!
//! The clock stays UTC and honest: the menu states the day, the date and the time in UTC and says that no
//! time zone is set; unanchored, it says the clock is not set and shows no date.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering};

use super::status::{self, Tray, ITEMS, ITEM_BATTERY, ITEM_CLOCK, ITEM_INPUT, ITEM_NET, ITEM_NONE, ITEM_VOLUME, NET_USB};
use super::{strip, theme};

/// A panel line starting with this byte is a SLIDER: `\x01NN`, NN = the level `00..=16`.
pub const SLIDER_MARK: u8 = 0x01;
/// A panel line starting with this byte is a GRID row of seven `|`-separated cells.
pub const GRID_MARK: u8 = 0x02;
/// The slider's scale (the volume model's `0..=16`).
const SLIDER_MAX: usize = 16;

const FACE: super::text::Face = super::text::Face::Chrome;

static OPEN_ITEM: AtomicU8 = AtomicU8::new(ITEM_NONE);
static ANCHOR_X: AtomicUsize = AtomicUsize::new(0);
/// Bumped on every open / pick: the crystal folds it into the dropdown's damage signature so a row whose
/// text changed (a level, a toggle) repaints while the panel stays open.
static GEN: AtomicU64 = AtomicU64::new(0);
static OPENS: AtomicU64 = AtomicU64::new(0);
static PICKS: AtomicU64 = AtomicU64::new(0);

/// The item whose menu is open, or [`ITEM_NONE`].
pub fn open_item() -> u8 {
    OPEN_ITEM.load(Ordering::Relaxed)
}

/// The panel-absolute x the open status menu hangs from (its item's left edge), or `None` (the battery
/// panel opened another way keeps its right-flush placement).
pub fn anchor_x() -> Option<usize> {
    if open_item() == ITEM_NONE { None } else { Some(ANCHOR_X.load(Ordering::Relaxed)) }
}

/// The repaint generation (see [`GEN`]).
pub fn generation() -> u64 {
    GEN.load(Ordering::Relaxed)
}

/// The crystal's dismiss calls this: whatever closed the panel, no status menu is open now.
pub fn closed() {
    OPEN_ITEM.store(ITEM_NONE, Ordering::Relaxed);
}

/// The status item under a panel point, when the bar draws it (an unknown item has no press cell).
pub fn item_at(pw: usize, ph: usize, px: usize, py: usize) -> Option<u8> {
    (0..ITEMS).find(|&i| {
        super::menubar::tray_box_abs(i, pw, ph).map(|(x, y, w, h)| px >= x && px < x + w && py >= y && py < y + h).unwrap_or(false)
    })
}

/// **Open item `i`'s menu** (the press on a status item). A menu already down — the SHARD menu or another
/// item's — is closed first, so the switch is one press, as winmenu's titles switch.
pub fn open(i: u8, pw: usize, ph: usize) -> bool {
    let Some((bx, _, _, _)) = super::menubar::tray_box_abs(i, pw, ph) else { return false };
    super::crystal::dismiss_for_switch();
    OPEN_ITEM.store(i, Ordering::Relaxed);
    ANCHOR_X.store(bx, Ordering::Relaxed);
    GEN.fetch_add(1, Ordering::Relaxed);
    OPENS.fetch_add(1, Ordering::Relaxed);
    let n = if i == ITEM_BATTERY {
        super::crystal::open_battery_panel(pw, ph); // POWERMENU M2's own path: its `:: POWER-UI:` line stands
        super::powerui::panel_rows()
    } else {
        let n = refresh();
        super::crystal::open_status_panel(pw, ph, via_word(i));
        n
    };
    serial_println!("[statusmenu] open item={} rows={} x={}", status::item_name(i), n, bx);
    true
}

fn via_word(i: u8) -> &'static str {
    match i {
        ITEM_INPUT => "status-input",
        ITEM_NET => "status-net",
        ITEM_VOLUME => "status-volume",
        ITEM_CLOCK => "status-clock",
        _ => "status-battery",
    }
}

/// Rebuild the open item's rows into the panel text. Returns the row count.
pub fn refresh() -> usize {
    let i = open_item();
    if i == ITEM_NONE {
        return 0;
    }
    let lines = lines_for(i, status::tray(), crate::clock::try_unix_now());
    super::powerui::panel_set_lines(lines)
}

/// **The rows of item `i`'s menu** — a pure function of the model (and the clock's seconds, for the clock).
pub fn lines_for(i: u8, t: Option<Tray>, now: Option<u64>) -> Vec<String> {
    let mut v = Vec::new();
    match i {
        ITEM_INPUT => {
            let pc = t.map(|t| t.pc).unwrap_or(false);
            v.push(String::from("Keyboard: U.S."));
            v.push(format!("Bindings: {}  Command = {}", super::keymap::active().name, if pc { "Alt" } else { "Cmd" }));
            v.push(String::from(if pc { "[x] PC modifiers (Alt as Command)" } else { "[ ] PC modifiers (Alt as Command)" }));
        }
        ITEM_NET => match t.and_then(|t| t.net) {
            None => v.push(String::from("No network device")),
            Some(n) => {
                if n.medium == NET_USB {
                    v.push(String::from("Ethernet (USB adapter)"));
                    v.push(String::from(if n.up { "Link: up" } else { "Link: down (no cable / no link)" }));
                } else {
                    v.push(String::from("Wi-Fi (radio up)"));
                    v.push(String::from("Not associated"));
                }
                match n.ip {
                    Some(a) => v.push(format!("Address: {}.{}.{}.{}", a[0], a[1], a[2], a[3])),
                    None => v.push(String::from("Address: none (no DHCP lease)")),
                }
                v.push(String::from("Turn Off (owed: no driver arm)"));
            }
        },
        ITEM_VOLUME => match t.and_then(|t| t.volume) {
            None => v.push(String::from("No audio output")),
            Some((lv, muted)) => {
                v.push(if muted { format!("Volume: {}/16 (muted)", lv) } else { format!("Volume: {}/16", lv) });
                let mut s = String::new();
                s.push(SLIDER_MARK as char);
                s.push((b'0' + (lv / 10) as u8) as char);
                s.push((b'0' + (lv % 10) as u8) as char);
                v.push(s);
                v.push(String::from(if muted { "Unmute" } else { "Mute" }));
                v.push(String::from("Output: HDA codec"));
            }
        },
        ITEM_BATTERY => v = super::powerui::battery_lines(status::reading().map(|(b, _)| b)),
        ITEM_CLOCK => match now {
            Some(secs) => clock_lines(secs, &mut v),
            None => {
                v.push(String::from("Clock not set"));
                v.push(String::from("No SNTP reply or RTC anchor yet"));
            }
        },
        _ => {}
    }
    v
}

const WEEKDAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// Sunday = 0 for a Unix day number (1970-01-01 was a Thursday).
fn dow(days: u64) -> usize {
    ((days + 4) % 7) as usize
}

fn month_len(y: i64, mo: u32) -> u32 {
    match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 29 } else { 28 },
    }
}

/// The clock's menu: the weekday, the long date, the time in UTC, then the month glance (a header row and
/// one GRID row per week, Sunday first; today's cell is marked).
pub fn clock_lines(secs: u64, v: &mut Vec<String>) {
    let (y, mo, d, h, mi, _) = crate::clock::civil_from_unix(secs);
    let days = secs / 86_400;
    let mo = mo.clamp(1, 12);
    v.push(String::from(WEEKDAYS[dow(days)]));
    v.push(format!("{} {} {}", d, MONTHS[(mo - 1) as usize], y));
    v.push(format!("{:02}:{:02} UTC (no time zone set)", h, mi));
    let mut hdr = String::new();
    hdr.push(GRID_MARK as char);
    hdr.push_str("Su|Mo|Tu|We|Th|Fr|Sa");
    v.push(hdr);
    let first_col = dow(days - (d as u64 - 1));
    let len = month_len(y, mo);
    let mut day = 1u32;
    let mut col = 0usize;
    while day <= len {
        let mut row = String::new();
        row.push(GRID_MARK as char);
        for c in 0..7 {
            if c > 0 {
                row.push('|');
            }
            let blank = (col == 0 && c < first_col) || day > len;
            if !blank {
                if day == d {
                    row.push('*');
                }
                row.push_str(&format!("{}", day));
                day += 1;
            }
        }
        col += 1;
        v.push(row);
    }
}

/// **A press inside the open status panel**: `row` is the panel line, `xin` the x inside the text area of
/// width `inner`. Acts through the owner's seam, rebuilds the rows, and keeps the panel open.
pub fn panel_press(row: usize, xin: usize, inner: usize) -> bool {
    let i = open_item();
    let t = status::tray();
    let what: &str = match (i, t) {
        (ITEM_VOLUME, Some(Tray { volume: Some((lv, muted)), .. })) => match row {
            1 if inner > 0 => {
                let nl = ((xin.min(inner) * SLIDER_MAX + inner / 2) / inner).min(SLIDER_MAX) as u8;
                let _ = status::set_volume(nl, false);
                "level"
            }
            2 => {
                let _ = status::set_volume(lv, !muted);
                if muted { "unmute" } else { "mute" }
            }
            _ => "none",
        },
        (ITEM_INPUT, Some(t)) if row == 2 => {
            let _ = super::keymap::set_pc(!t.pc);
            if t.pc { "mac-table" } else { "pc-table" }
        }
        _ => "none",
    };
    if what != "none" {
        PICKS.fetch_add(1, Ordering::Relaxed);
        GEN.fetch_add(1, Ordering::Relaxed);
        refresh();
        let (lv, m) = status::volume();
        serial_println!("[statusmenu] pick item={} row={} -> {} volume={}/16 muted={} table={}", status::item_name(i), row, what, lv, m as u8, super::keymap::active().name);
    }
    what != "none"
}

// ── The panel's two drawn row kinds ──────────────────────────────────────────────────────────────

/// Paint one scanline of a SLIDER row: a track across `[x0, x0 + inner)` filled to the level, centred in the
/// `item_h`-tall band (`sy` = the row inside the band).
pub fn paint_slider(out: &mut [u32], w: usize, x0: usize, inner: usize, sy: usize, item_h: usize, line: &[u8]) {
    let lv = if line.len() >= 3 { ((line[1] - b'0') as usize * 10 + (line[2] - b'0') as usize).min(SLIDER_MAX) } else { 0 };
    let th = crate::ui::px(4).max(2);
    let top = item_h.saturating_sub(th) / 2;
    if sy < top || sy >= top + th {
        return;
    }
    let fill = inner * lv / SLIDER_MAX;
    for k in 0..inner {
        let i = x0 + k;
        if i < w {
            out[i] = if k < fill { theme::accent() } else { theme::frame_line() };
        }
    }
}

/// Paint one glyph scanline (`sy` inside the cell, `cell_h` tall) of a GRID row: seven columns across
/// `[x0, x0 + inner)`, each cell right-aligned in its column; today's cell sits on the accent box.
pub fn paint_grid(out: &mut [u32], w: usize, x0: usize, inner: usize, sy: usize, cells: &[u8]) {
    let cw = inner / 7;
    for (c, cell) in cells.split(|&b| b == b'|').enumerate().take(7) {
        let (today, txt) = match cell.first() {
            Some(b'*') => (true, &cell[1..]),
            _ => (false, cell),
        };
        if txt.is_empty() {
            continue;
        }
        let right = x0 + (c + 1) * cw - strip::PAD() / 2;
        let tw = super::text::advance(txt, false, FACE);
        let tx = right.saturating_sub(tw);
        let ink = if today {
            let bx0 = tx.saturating_sub(strip::PAD() / 2);
            for i in bx0..(right + strip::PAD() / 2).min(w) {
                out[i] = theme::accent();
            }
            theme::bevel_light()
        } else {
            theme::title_text_active()
        };
        super::text::draw_row(out, w, txt, tx, sy, ink, false, FACE);
    }
}

// ── The witness ────────────────────────────────────────────────────────────────────────────────

static WIRE_KEY: AtomicU64 = AtomicU64::new(u64::MAX);

/// `:: STATUSTRAY: items=5 drawn=<n> menus=<n> volume=<v/16|muted|none> net=<up|down|none> input=<us|pc>
/// clock=<synced|unsynced> ::` — from the bar's paint, once per STATE (which items are drawn, mute, link,
/// the table, the anchor), never per level tick. `drawn` counts the items the bar seated; `menus` the press
/// cells that open one (every drawn item has a menu).
pub fn witness(t: Option<Tray>, pw: usize, ph: usize) {
    let Some(t) = t else { return };
    let mut drawn = 0u64;
    let mut mask = 0u64;
    for i in 0..ITEMS {
        if super::menubar::tray_box_abs(i, pw, ph).is_some() {
            drawn += 1;
            mask |= 1 << i;
        }
    }
    let vol_k = match t.volume { None => 0, Some((_, true)) => 1, Some(_) => 2 };
    let net_k = match t.net { None => 0, Some(n) if n.up => 2, Some(_) => 1 };
    let key = mask | (vol_k << 8) | (net_k << 10) | ((t.pc as u64) << 12) | ((t.clock as u64) << 13);
    if WIRE_KEY.swap(key, Ordering::Relaxed) == key {
        return;
    }
    let vol = match t.volume {
        None => String::from("none"),
        Some((_, true)) => String::from("muted"),
        Some((lv, false)) => format!("{}/16", lv),
    };
    crate::census_println!(
        ":: STATUSTRAY: items={} drawn={} menus={} volume={} net={} input={} clock={} ::",
        ITEMS, drawn, drawn, vol,
        match t.net { None => "none", Some(n) if n.up => "up", Some(_) => "down" },
        if t.pc { "pc" } else { "us" },
        if t.clock { "synced" } else { "unsynced" }
    );
}

// ── `tests statustray` (R80: registered from the desktop pass, never run at boot) ─────────────────

/// Register `tests statustray` once (from `status::tray_publish`, the desktop pass).
pub fn ensure_registered() {
    #[cfg(feature = "witness")]
    {
        static DONE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
        if !DONE.swap(true, Ordering::AcqRel) {
            crate::tests::register("statustray", fixture);
        }
    }
}

/// `tests statustray`: the model's facts injected (a sink, a dongle with link and a lease, the anchor),
/// every item's rows built from them, the volume slider and mute and the input toggle driven through
/// [`panel_press`] (the press path's own function) and read back from the OWNERS (`status::volume`,
/// `keymap::active`), the clock's rows checked on a fixed day, the unknown items reported unknown; then the
/// model, the level, the mute and the table restored.
#[cfg(feature = "witness")]
pub fn fixture() {
    let raw = status::tray_raw();
    let (l0, m0) = status::volume();
    let pc0 = super::keymap::pc_selected();
    let item0 = open_item();
    // a sink, a USB link that is up, a lease of 10.0.1.2, the anchor.
    let net = NET_USB as u64 | (1 << 8) | (1 << 9) | ((u32::from_be_bytes([10, 0, 1, 2]) as u64) << 16);
    status::tray_inject(true, net, true);
    let _ = status::set_volume(12, false);
    let t = status::tray();
    let vl = lines_for(ITEM_VOLUME, t, None);
    let volume_ok = vl.len() == 4 && vl[0] == "Volume: 12/16" && vl[1].as_bytes() == [SLIDER_MARK, b'1', b'2'];
    let nl = lines_for(ITEM_NET, t, None);
    let net_ok = nl.len() == 4 && nl[1] == "Link: up" && nl[2] == "Address: 10.0.1.2";
    // the press path: the volume menu open, the slider pressed at its middle, then Mute.
    OPEN_ITEM.store(ITEM_VOLUME, Ordering::Relaxed);
    let slid = panel_press(1, 50, 100) && status::volume() == (8, false);
    let muted = panel_press(2, 0, 100) && status::volume() == (8, true);
    OPEN_ITEM.store(ITEM_INPUT, Ordering::Relaxed);
    let _ = super::keymap::set_pc(false);
    let input_ok = panel_press(2, 0, 100) && super::keymap::active().name == "pc" && panel_press(2, 0, 100) && super::keymap::active().name == "crispy";
    // the clock on 2026-10-06 09:41 UTC: a Tuesday; the 1st is a Thursday (column 4); today marked.
    let mut cl = Vec::new();
    clock_lines(1_791_279_660, &mut cl);
    let clock_ok = cl.len() >= 9
        && cl[0] == "Tuesday"
        && cl[1] == "6 October 2026"
        && cl[2] == "09:41 UTC (no time zone set)"
        && cl[4].as_bytes()[1..] == *b"||||1|2|3"
        && cl[5].as_bytes()[1..] == *b"4|5|*6|7|8|9|10";
    let unset_ok = lines_for(ITEM_CLOCK, t, None)[0] == "Clock not set";
    // unknown facts: no sink, no device -> the items are unknown (not drawn) and their menus say so.
    status::tray_inject(false, 0, false);
    let t2 = status::tray().unwrap_or(Tray { volume: None, net: None, pc: false, clock: false });
    let absent_ok = !status::item_known(&t2, ITEM_VOLUME) && !status::item_known(&t2, ITEM_NET) && status::item_known(&t2, ITEM_INPUT);
    // restore
    status::tray_restore(raw);
    let _ = status::set_volume(l0, m0);
    if super::keymap::pc_selected() != pc0 {
        let _ = super::keymap::set_pc(pc0);
    }
    OPEN_ITEM.store(item0, Ordering::Relaxed);
    let ok = volume_ok && net_ok && slid && muted && input_ok && clock_ok && unset_ok && absent_ok;
    let w = |b: bool| if b { "ok" } else { "no" };
    serial_println!(
        ":: STATUSTRAY-T: volume={} slider={} mute={} net={} input={} clock={} unset={} absent={} opens={} picks={} -> {} ::",
        w(volume_ok), w(slid), w(muted), w(net_ok), w(input_ok), w(clock_ok), w(unset_ok), w(absent_ok),
        OPENS.load(Ordering::Relaxed), PICKS.load(Ordering::Relaxed), if ok { "PASS" } else { "FAIL" }
    );
}
