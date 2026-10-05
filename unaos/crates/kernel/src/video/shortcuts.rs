// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// SHORTCUTS (R75) — ONE table of every chord the desktop answers, and the help overlay that lists it.
//
// M1: [`SHORTCUTS`] is the table. The `C_*` constants are the chord TOKENS the keymap rows
//     (`theme.rs`) carry; those rows read the constants here, so the token a row names and the chord
//     this table lists are one string. Chords decoded by a literal that cannot be swapped on one line
//     are listed in `docs/dev/evidence/rmbp-0929/SHORTCUTS.md` (§Duplication).
// M2: [`open`] / [`close`] / [`overlay_key`] / [`overlay_dismiss`] — a chromeless topmost compat row
//     (`wm::overlay_open`, the SPLASHX86 pattern), two columns grouped by scope, closed by any key or
//     click. The pure [`layout`] is what the witness counts, so it runs with no compositor.
// M3: [`chord_for`] — the app menu rows ask the table for the chord to draw on their right.

use alloc::string::String;
use alloc::vec::Vec;

/// One chord the desktop answers.
#[derive(Clone, Copy)]
pub struct Shortcut {
    /// The chord: the keymap token (`cmd-l`) where a keymap row decodes it, display text otherwise.
    pub chord: &'static str,
    /// Where it applies (the help overlay groups by this).
    pub scope: &'static str,
    /// What it does; also the label an app-menu row matches in [`chord_for`].
    pub action: &'static str,
    /// The arc that built it.
    pub arc: &'static str,
}

// Keymap tokens, read by `theme.rs`'s rows (CRISPY and PC tables) in place of their own literals.
pub const C_ALT_TAB: &str = "alt-tab";
pub const C_CMD_TAB: &str = "cmd-tab";
pub const C_CMD_L: &str = "cmd-l";
pub const C_CTRL_ALT_L: &str = "ctrl-alt-l";
pub const C_CMD_SHIFT_3: &str = "cmd-shift-3";
pub const C_CMD_SHIFT_4: &str = "cmd-shift-4";
pub const C_PRINT_SCREEN: &str = "print-screen";
pub const C_CMD_SLASH: &str = "cmd-/";
pub const C_CMD_M: &str = "cmd-m";
pub const C_CMD_GRAVE: &str = "cmd-`";
pub const C_CMD_K: &str = "cmd-k"; // LUMENBIN

const fn s(chord: &'static str, scope: &'static str, action: &'static str, arc: &'static str) -> Shortcut {
    Shortcut { chord, scope, action, arc }
}

/// The table. Order is display order within a scope; scopes appear in first-use order.
pub static SHORTCUTS: &[Shortcut] = &[
    s(C_CMD_SLASH, "Desktop", "Keyboard Shortcuts", "SHORTCUTS"),
    s(C_ALT_TAB, "Desktop", "Next window", "WINCYCLE"),
    s(C_CMD_TAB, "Desktop", "Next window", "WINCYCLE"),
    s(C_CMD_L, "Desktop", "Lock screen", "SCREENLOCK"),
    s(C_CTRL_ALT_L, "Desktop", "Lock screen", "SCREENLOCK"),
    s("F1 / F2", "Desktop", "Brightness down / up", "BRIGHTKEYS"),
    s("F10 / F11 / F12", "Desktop", "Volume", "AUDIOKEYS"),
    s(C_CMD_SHIFT_3, "Capture", "Screenshot", "PRTSCR"),
    s(C_CMD_SHIFT_4, "Capture", "Screenshot region", "SHOTREGION"),
    s(C_PRINT_SCREEN, "Capture", "Screenshot", "PRTSCR"),
    s("title double-click", "Window", "Zoom / restore", "WINCYCLE"),
    s(C_CMD_M, "Window", "Minimize", "WINDOWLIST"),
    s(C_CMD_GRAVE, "Window", "Next window of app", "WINDOWLIST"),
    s("cmd-alt-left", "Window", "Snap Left", "WINSNAP"),
    s("cmd-alt-right", "Window", "Snap Right", "WINSNAP"),
    s("right-click tile", "Dock", "Tile menu (Quit)", "DOCKRUN"),
    s("right-click entry", "Quarry", "File operations", "QUARRYOPS"),
    s("Ctrl-C", "Shell", "Cancel line", "SHELLUX"),
    s("Ctrl-L", "Shell", "Clear screen", "SHELLUX"),
    s("Ctrl-A / Ctrl-E", "Shell", "Line start / end", "SHELLUX"),
    s("Ctrl-U", "Shell", "Kill to line start", "SHELLUX"),
    s("Ctrl-W", "Shell", "Kill word", "SHELLUX"),
    s("Tab", "Shell", "Complete", "SHELLUX"),
    s("Ctrl-S", "Editor", "Save", "TEXTEDIT"),
    s("q", "Activity", "Quit", "ACTIVITY"),
    s("k", "Activity", "Kill selected", "ACTIVITY"),
    s(C_CMD_K, "Lumen", "Clear transcript", "LUMENBIN"),
];

/// The chord an app-menu row labelled `label` shows on its right (first table entry whose action matches).
pub fn chord_for(label: &str) -> Option<&'static str> {
    SHORTCUTS.iter().find(|e| e.action == label).map(|e| e.chord)
}

/// Distinct scopes, in first-use order.
pub fn scope_count() -> usize {
    let mut n = 0usize;
    for (i, e) in SHORTCUTS.iter().enumerate() {
        if !SHORTCUTS[..i].iter().any(|p| p.scope == e.scope) {
            n += 1;
        }
    }
    n
}

/// One drawn line of the overlay.
pub struct Line {
    pub col: usize,
    pub row: usize,
    pub header: bool,
    pub text: String,
}

/// Chord field width on an entry line, in glyphs.
const CHORD_W: usize = 18;

/// Lay the table out in two columns, grouped by scope. Returns the lines and the rows of the taller column.
pub fn layout() -> (Vec<Line>, usize) {
    // Flatten to (header?, text) in order.
    let mut flat: Vec<(bool, String)> = Vec::new();
    for (i, e) in SHORTCUTS.iter().enumerate() {
        if SHORTCUTS[..i].iter().any(|p| p.scope == e.scope) {
            continue;
        }
        flat.push((true, String::from(e.scope)));
        for f in SHORTCUTS.iter().filter(|f| f.scope == e.scope) {
            let mut t = String::from(f.chord);
            while t.len() < CHORD_W {
                t.push(' ');
            }
            t.push_str(f.action);
            flat.push((false, t));
        }
    }
    // Break at the first group start at or past half the lines.
    let half = (flat.len() + 1) / 2;
    let mut brk = flat.len();
    for (i, f) in flat.iter().enumerate() {
        if f.0 && i >= half {
            brk = i;
            break;
        }
    }
    let mut out = Vec::new();
    let (mut r0, mut r1) = (0usize, 0usize);
    for (i, (h, t)) in flat.into_iter().enumerate() {
        if i < brk {
            out.push(Line { col: 0, row: r0, header: h, text: t });
            r0 += 1;
        } else {
            out.push(Line { col: 1, row: r1, header: h, text: t });
            r1 += 1;
        }
    }
    (out, core::cmp::max(r0, r1))
}

/// Entry lines in a layout (what the witness calls `shown`).
pub fn shown(lines: &[Line]) -> usize {
    lines.iter().filter(|l| !l.header).count()
}

/// Print the table to the console (the `shortcuts` verb).
pub fn shell_verb(console: &mut crate::console::Console) {
    let (lines, _) = layout();
    for l in lines.iter() {
        if l.header {
            console.println(&alloc::format!("{}:", l.text));
        } else {
            console.println(&alloc::format!("  {}", l.text));
        }
    }
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    let _ = open();
}

#[cfg(all(target_arch = "x86_64", feature = "wc"))]
mod ov {
    use super::*;
    use core::sync::atomic::{AtomicU32, Ordering};
    pub static WIN: AtomicU32 = AtomicU32::new(0);
    pub static SURF: spin::Mutex<Option<Vec<u32>>> = spin::Mutex::new(None);
}

/// Is the overlay up?
pub fn is_open() -> bool {
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    {
        return ov::WIN.load(core::sync::atomic::Ordering::Acquire) != 0;
    }
    #[allow(unreachable_code)]
    false
}

/// Open the overlay (idempotent). `false` on refusal (no panel, allocation, window table).
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub fn open() -> bool {
    use crate::video::{menubar, theme};
    use core::sync::atomic::Ordering;
    if is_open() {
        return true;
    }
    let (pw, ph) = {
        let fb = *crate::video::WRITER.lock();
        if !fb.is_ready() {
            return false;
        }
        let i = fb.info();
        (i.width, i.height)
    };
    let (lines, rows) = layout();
    let (cw, ch) = (menubar::BAR_CELL_W, menubar::BAR_CELL_H);
    let lh = ch + 4;
    let colw = lines.iter().map(|l| l.text.len()).max().unwrap_or(0) * cw;
    let pad = 16usize;
    let w = (2 * colw + 3 * pad).min(pw);
    let h = ((rows + 2) * lh + 2 * pad).min(ph);
    let mut buf: Vec<u32> = Vec::new();
    if buf.try_reserve_exact(w * h).is_err() {
        return false;
    }
    buf.resize(w * h, theme::CHROME_FACE);
    for x in 0..w {
        buf[x] = theme::FRAME_LINE;
        buf[(h - 1) * w + x] = theme::FRAME_LINE;
    }
    for y in 0..h {
        buf[y * w] = theme::FRAME_LINE;
        buf[y * w + w - 1] = theme::FRAME_LINE;
    }
    let draw = |buf: &mut [u32], text: &[u8], x: usize, top: usize, bold: bool| {
        for sy in 0..ch {
            let y = top + sy;
            if y < h {
                super::text::draw_row(&mut buf[y * w..(y + 1) * w], w, text, x, sy, theme::TITLE_TEXT_ACTIVE, bold, menubar::BAR_FACE);
            }
        }
    };
    draw(&mut buf, b"Keyboard Shortcuts   (any key or click closes)", pad, pad, true);
    for l in lines.iter() {
        let x = pad + l.col * (colw + pad);
        let top = pad + (l.row + 2) * lh;
        draw(&mut buf, l.text.as_bytes(), x, top, l.header);
    }
    let addr = buf.as_ptr() as usize;
    let len = buf.len() * 4;
    *ov::SURF.lock() = Some(buf);
    let id = crate::video::wm::overlay_open(addr, len, w, h, pw.saturating_sub(w) / 2, ph.saturating_sub(h) / 3);
    if id == crate::video::wm::WIN_NONE {
        *ov::SURF.lock() = None;
        return false;
    }
    crate::video::wm::set_modal_top(id);
    ov::WIN.store(id, Ordering::Release);
    serial_println!("[shortcuts] overlay OPEN win={} {}x{} entries={}", id, w, h, shown(&lines));
    true
}
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
pub fn open() -> bool { false }

/// Close the overlay (idempotent).
pub fn close() -> bool {
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    {
        let id = ov::WIN.swap(0, core::sync::atomic::Ordering::AcqRel);
        if id == 0 {
            return false;
        }
        crate::video::wm::clear_modal_top(id);
        crate::video::wm::close(id);
        *ov::SURF.lock() = None; // after the row is gone
        serial_println!("[shortcuts] overlay CLOSE win={}", id);
        return true;
    }
    #[allow(unreachable_code)]
    false
}

/// The key door: while the overlay is up ANY key (or action) closes it and is consumed; a key-up is swallowed.
pub fn overlay_key(ev: crate::pal::Event) -> bool {
    if !is_open() {
        return false;
    }
    match ev {
        crate::pal::Event::Key(_) | crate::pal::Event::Action(_) => {
            close();
            true
        }
        crate::pal::Event::KeyUp(_) => true,
        _ => false,
    }
}

/// The click door: a press while the overlay is up closes it and is consumed.
pub fn overlay_dismiss() -> bool {
    is_open() && close()
}

/// The fixture (`tests shortcuts`; also the boot witness): count the table, lay it out, and — with the
/// compositor — open the overlay and close it through the key door.
pub fn selftest() {
    let entries = SHORTCUTS.len();
    let scopes = scope_count();
    let (lines, _) = layout();
    let shown_n = shown(&lines);
    let mut ok = entries >= 12 && scopes >= 5 && shown_n == entries && chord_for("Keyboard Shortcuts") == Some(C_CMD_SLASH);
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    {
        let opened = open();
        let up = is_open();
        let eaten = overlay_key(crate::pal::Event::Key(b' '));
        let closed = !is_open();
        serial_println!("[shortcuts] fixture opened={} up={} key_eaten={} closed={}", opened, up, eaten, closed);
        // A compositor with no panel (headless) declines the open; that is not a table failure.
        if opened {
            ok = ok && up && eaten && closed;
        }
    }
    serial_println!(":: SHORTCUTS: entries={} scopes={} shown={} -> {} ::", entries, scopes, shown_n, if ok { "PASS" } else { "FAIL" });
}
