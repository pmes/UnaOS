// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Matrix — kernel-by-ruling R50
//!
//! ATTRCOLUMNS (rmbp-ledger B402) — Quarry's GET INFO, first form (MACPARITY row 29): an inspector pane over the
//! list listing EVERY attribute of the selected file — key, type, value, and the time it changed (from
//! `una:attrtimes`, `-` when no writer recorded one) — under the file's kind, size and modified time. Opened from
//! the right-click menu (`Get Info`), the `i` key, or Cmd-I (`Action::GetInfo`); Esc or any press dismisses.
//! A volume without typed attributes (FAT) says so. Read-only: the edit is the column's (`attrcols`).

use super::*;
use crate::fs::attrfacts as af;
use crate::fs::vfs::{VfsError, KERNEL_PRINCIPAL};
use core::sync::atomic::AtomicBool;

struct Panel {
    path: String,
    head: String,
    rows: Vec<af::InfoRow>,
    note: Option<&'static str>,
}

static PANEL: spin::Mutex<Option<Panel>> = spin::Mutex::new(None);
static UP: AtomicBool = AtomicBool::new(false);

pub fn is_up() -> bool {
    UP.load(Ordering::Acquire)
}

pub fn dismiss() {
    *PANEL.lock() = None;
    UP.store(false, Ordering::Release);
}

/// Open the inspector for `path` (no Quarry lock held). Prints `[getinfo] path=… attrs=<n>` and one line per row.
pub fn open(path: &str) {
    let mt = crate::shell::vfs_mount_table();
    let st = match mt.stat(path) {
        Ok(s) => s,
        Err(e) => {
            serial_println!("[getinfo] path={} refused={:?}", path, e);
            return;
        }
    };
    let kind = if matches!(st.kind, NodeKind::Dir) { "folder" } else { "file" };
    let when = st.mtime.map(af::fmt_when).unwrap_or_else(|| String::from("-"));
    let head = alloc::format!("{}  {} bytes  modified {}", kind, st.size, when);
    let (rows, note) = match af::info_rows(&mt, path) {
        Ok(r) => (r, None),
        Err(VfsError::Unsupported) => (Vec::new(), Some("this volume carries no typed attributes (FAT)")),
        Err(_) => (Vec::new(), Some("attributes unreadable")),
    };
    serial_println!("[getinfo] path={} attrs={} {}", path, rows.len(), head);
    for r in rows.iter() {
        serial_println!("[getinfo]   {} ({}) = {} changed={}", r.key, r.ty, r.val, r.when.map(af::fmt_when).unwrap_or_else(|| String::from("-")));
    }
    let _ = KERNEL_PRINCIPAL;
    *PANEL.lock() = Some(Panel { path: String::from(path), head, rows, note });
    UP.store(true, Ordering::Release);
}

/// The inspector over the list pane (called from `ops::paint_overlay`, the model lock held).
pub(super) fn paint(m: &Model, px: &mut [u32]) {
    let guard = PANEL.lock();
    let Some(p) = guard.as_ref() else { return };
    let g = &m.geom;
    let li = g.list_pane().inner();
    let rh = g.row_h();
    let n = 3 + p.rows.len().max(1);
    let h = (n * rh + 2).min(li.h);
    let (x, y, w) = (li.x + PAD(), li.y + rh, li.w.saturating_sub(2 * PAD()));
    fill(px, g, x, y, w, h, theme::BUTTON_FACE);
    keyline(px, g, Rect { x, y, w, h }, theme::ACCENT);
    let clip = x + w;
    let mut line = |i: usize, s: &str, ink: u32| {
        if (i + 1) * rh <= h {
            let b: Vec<u8> = s.bytes().map(|c| if (0x20..0x7f).contains(&c) { c } else { b'?' }).collect();
            text(px, g, x + PAD(), y + 1 + i * rh + g.ts, &b, clip, ink);
        }
    };
    line(0, &alloc::format!("Get Info: {}", p.path), theme::BUTTON_TEXT);
    line(1, &p.head, theme::TITLE_TEXT_INACTIVE);
    line(2, "attribute                type    value                         changed", theme::TITLE_TEXT_INACTIVE);
    if let Some(note) = p.note {
        line(3, note, theme::BUTTON_TEXT);
    } else if p.rows.is_empty() {
        line(3, "no attributes", theme::BUTTON_TEXT);
    }
    for (i, r) in p.rows.iter().enumerate() {
        let mut v = r.val.clone();
        v.truncate(29);
        let s = alloc::format!("{:<24} {:<7} {:<29} {}", r.key, r.ty, v, r.when.map(af::fmt_when).unwrap_or_else(|| String::from("-")));
        line(3 + i, &s, theme::BUTTON_TEXT);
    }
}
