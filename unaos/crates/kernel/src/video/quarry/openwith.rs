// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Matrix — kernel-by-ruling R50
//!
//! FILETYPES (rmbp-ledger B423, MACPARITY §16 B3 / row 29) — Quarry's OPEN WITH: the right-click menu's
//! `Open With…` row opens a list of EVERY registrant of the selected file's type (`assoc::registrants_in`: the
//! programs whose resources declare the type, then APPRES's ring-3 programs), the one that opens it by default
//! marked `(default)`. A pick opens the file with that program through THE one dispatch (`openers::act` →
//! `openers::open`); the default does not change (Settings → File Types changes the type's, `setfattr
//! <file> una:preferred=<signature>` one file's). A type nothing declares lists `(no app opens <type>)`.
//!
//! Wire: `[quarry] openwith path=<p> type=<m> registrants=<n> default=<opener>` on open,
//! `[quarry] openwith pick=<opener> path=<p>` on a pick.

use super::*;
use crate::fs::appres::Registrant;
use core::sync::atomic::AtomicBool;

struct List {
    x: usize,
    y: usize,
    path: String,
    mime: String,
    rows: Vec<Registrant>,
    default: String,
}

static LIST: spin::Mutex<Option<List>> = spin::Mutex::new(None);
static UP: AtomicBool = AtomicBool::new(false);

pub fn is_up() -> bool {
    UP.load(Ordering::Acquire)
}

pub fn dismiss() {
    *LIST.lock() = None;
    UP.store(false, Ordering::Release);
}

/// The registrants of `path`'s type and the default opener (no Quarry lock held). Pure apart from the VFS reads.
pub fn candidates(path: &str) -> (String, Vec<Registrant>, String) {
    let mt = crate::shell::vfs_mount_table();
    let (mime, _) = crate::fs::filetype::type_of_in(&mt, path);
    let rows = crate::fs::assoc::registrants_in(&mt, &mime);
    let (default, _) = crate::fs::assoc::opener_for_in(&mt, path, &mime);
    (mime, rows, default)
}

/// Open the list for `path` at source point (`x`, `y`) — the context menu's origin.
pub fn open(path: &str, x: usize, y: usize) {
    let (mime, rows, default) = candidates(path);
    serial_println!("[quarry] openwith path={} type={} registrants={} default={}", path, mime, rows.len(), default);
    *LIST.lock() = Some(List { x, y, path: String::from(path), mime, rows, default });
    UP.store(true, Ordering::Release);
}

fn width(g: &Geom) -> usize {
    30 * g.cell_w() + 2 * PAD()
}

/// The list over the frame (called from `ops::paint_overlay`, the model lock held).
pub(super) fn paint(m: &Model, px: &mut [u32]) {
    let guard = LIST.lock();
    let Some(l) = guard.as_ref() else { return };
    let g = &m.geom;
    let rh = g.row_h();
    let n = 1 + l.rows.len().max(1);
    let w = width(g);
    let h = n * rh + 2;
    let x = l.x.min(g.w.saturating_sub(w));
    let y = l.y.min(g.h.saturating_sub(h));
    fill(px, g, x, y, w, h, theme::button_face());
    keyline(px, g, Rect { x, y, w, h }, theme::accent());
    let head = alloc::format!("Open With ({})", l.mime);
    text(px, g, x + PAD(), y + 1 + g.ts, head.as_bytes(), x + w, theme::content_text());
    if l.rows.is_empty() {
        let s = alloc::format!("(no app opens {})", l.mime);
        text(px, g, x + PAD(), y + 1 + rh + g.ts, s.as_bytes(), x + w, theme::button_text());
    }
    for (i, r) in l.rows.iter().enumerate() {
        let mut s = r.name.clone();
        if r.opener == l.default {
            s.push_str("  (default)");
        }
        if !super::openers::available(&r.opener) {
            s.push_str("  (not in this build)");
        }
        text(px, g, x + PAD(), y + 1 + (i + 1) * rh + g.ts, s.as_bytes(), x + w, theme::button_text());
    }
}

/// A primary press while the list is up (`hit` = the source point inside Quarry, if any): a row opens the file
/// with that registrant; anything else dismisses. Returns the act to run outside the model lock.
pub(super) fn press(hit: Option<(usize, usize)>, g: &Geom) -> Act {
    let l = LIST.lock().take();
    UP.store(false, Ordering::Release);
    let (Some(l), Some((sx, sy))) = (l, hit) else { return Act::None };
    let rh = g.row_h();
    let (w, h) = (width(g), (1 + l.rows.len().max(1)) * rh + 2);
    let x = l.x.min(g.w.saturating_sub(w));
    let y = l.y.min(g.h.saturating_sub(h));
    if sx < x || sx >= x + w || sy < y + 1 + rh {
        return Act::None;
    }
    let i = (sy - y - 1) / rh - 1;
    let Some(r) = l.rows.get(i) else { return Act::None };
    serial_println!("[quarry] openwith pick={} path={}", r.opener, l.path);
    super::openers::act(r.opener.clone(), l.path, l.mime)
}
