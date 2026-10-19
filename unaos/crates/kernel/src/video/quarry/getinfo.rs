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
    /// QUARRYVERSIONS (B732): `Some` = the panel is the file's version list (`(seq, replaced-at, size)`, newest
    /// first), not its attributes.
    versions: Option<Vec<(u64, u64, u64)>>,
}

static PANEL: crate::sync::Mutex<Option<Panel>> = crate::sync::Mutex::new(None);
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
    let (mut rows, note) = match af::info_rows(&mt, path) {
        Ok(r) => (r, None),
        Err(VfsError::Unsupported) => (Vec::new(), Some("this volume carries no typed attributes (FAT)")),
        Err(_) => (Vec::new(), Some("attributes unreadable")),
    };
    // SMALLFIX3 (B416): a settings file (SETTINGSFILES B407, R98) shows its auto-saved line and every key in the
    // SAME inspector — one Get Info, not a second notice (`ops.rs`'s Show Info joins here).
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    for l in crate::video::settingsfiles::info_lines(path) {
        rows.push(af::InfoRow { key: l, ty: "setting", val: String::new(), when: None });
    }
    serial_println!("[getinfo] path={} attrs={} {}", path, rows.len(), head);
    for r in rows.iter() {
        serial_println!("[getinfo]   {} ({}) = {} changed={}", r.key, r.ty, r.val, r.when.map(af::fmt_when).unwrap_or_else(|| String::from("-")));
    }
    let _ = KERNEL_PRINCIPAL;
    *PANEL.lock() = Some(Panel { path: String::from(path), head, rows, note, versions: None });
    UP.store(true, Ordering::Release);
}

/// The inspector over the list pane (called from `ops::paint_overlay`, the model lock held).
pub(super) fn paint(m: &Model, px: &mut [u32]) {
    let guard = PANEL.lock();
    let Some(p) = guard.as_ref() else { return };
    if let Some(v) = p.versions.as_ref() {
        return paint_versions(m, px, p, v); // QUARRYVERSIONS (B732)
    }
    let g = &m.geom;
    let li = g.list_pane().inner();
    let rh = g.row_h();
    let n = 3 + p.rows.len().max(1);
    let h = (n * rh + 2).min(li.h);
    let (x, y, w) = (li.x + PAD(), li.y + rh, li.w.saturating_sub(2 * PAD()));
    fill(px, g, x, y, w, h, theme::surface_raised());
    keyline(px, g, Rect { x, y, w, h }, theme::accent());
    let clip = x + w;
    let mut line = |i: usize, s: &str, ink: u32| {
        if (i + 1) * rh <= h {
            let b: Vec<u8> = s.bytes().map(|c| if (0x20..0x7f).contains(&c) { c } else { b'?' }).collect();
            text(px, g, x + PAD(), y + 1 + i * rh + g.ts, &b, clip, ink);
        }
    };
    line(0, &alloc::format!("Get Info: {}", p.path), theme::text_primary());
    line(1, &p.head, theme::text_secondary());
    line(2, "attribute                type    value                         changed", theme::text_secondary());
    if let Some(note) = p.note {
        line(3, note, theme::text_primary());
    } else if p.rows.is_empty() {
        line(3, "no attributes", theme::text_primary());
    }
    for (i, r) in p.rows.iter().enumerate() {
        let mut v = r.val.clone();
        v.truncate(29);
        let s = alloc::format!("{:<24} {:<7} {:<29} {}", r.key, r.ty, v, r.when.map(af::fmt_when).unwrap_or_else(|| String::from("-")));
        line(3 + i, &s, theme::text_primary());
    }
}

// ── QUARRYVERSIONS (rmbp-ledger B732): the version list in the same inspector ─────────────────────────────────────
//
// 'Previous versions…' (ops' menu row) opens THIS panel on the file's version ring (FILEVERSIONS B714): the file's
// head line, then one row per version, newest first — the date it was replaced (`clock::locale::fmt_stamp`), its
// size, and two keyline buttons, Restore and Open (no `accent.fill`: neither is the view's default, BP-video-17). A
// file with none says 'No previous versions'; a folder never opens it (ops refuses first, and so does
// [`open_versions`]). Esc or a press anywhere else dismisses, as Get Info; a button press runs `ops::version_press`.

/// QUARRYVERSIONS (B732): open the inspector on the versions of the FILE at `path` (no Quarry lock held).
pub fn open_versions(path: &str) {
    let mt = crate::shell::vfs_mount_table();
    let st = match mt.stat(path) {
        Ok(s) if !matches!(s.kind, NodeKind::Dir) => s,
        Ok(_) => {
            serial_println!("[quarry] versions path={} refused=folder", path);
            return;
        }
        Err(e) => {
            serial_println!("[quarry] versions path={} refused={:?}", path, e);
            return;
        }
    };
    let when = st.mtime.map(super::ops::version_date).unwrap_or_else(|| String::from("-"));
    let head = alloc::format!("file  {} bytes  modified {}", st.size, when);
    let (rows, note) = match super::ops::version_rows(path) {
        Ok(v) => (v, None),
        Err(why) => (Vec::new(), Some(why)),
    };
    serial_println!("[quarry] versions path={} native={} listed={}", path, super::ops::native_path(path).as_deref().unwrap_or("-"), rows.len());
    *PANEL.lock() = Some(Panel { path: String::from(path), head, rows: Vec::new(), note, versions: Some(rows) });
    UP.store(true, Ordering::Release);
}

/// QUARRYVERSIONS (B732): how many version rows the open panel lists (`None`: no version list is up).
pub fn version_count() -> Option<usize> {
    PANEL.lock().as_ref().and_then(|p| p.versions.as_ref().map(|v| v.len()))
}

/// QUARRYVERSIONS (B732): the `seq` of version row `i` of the open list.
pub fn version_seq(i: usize) -> Option<u64> {
    PANEL.lock().as_ref().and_then(|p| p.versions.as_ref().and_then(|v| v.get(i).map(|r| r.0)))
}

/// The panel's frame over the list pane, shared by the painter and the press: `(x, y, w, h, rh)`.
fn frame(g: &Geom, rows: usize) -> (usize, usize, usize, usize, usize) {
    let li = g.list_pane().inner();
    let rh = g.row_h();
    let h = ((3 + rows.max(1)) * rh + 2).min(li.h);
    (li.x + PAD(), li.y + rh, li.w.saturating_sub(2 * PAD()), h, rh)
}

/// A version row's buttons: `(restore x, open x, button w)`, right-aligned in the panel.
fn buttons(g: &Geom, x: usize, w: usize) -> (usize, usize, usize) {
    let bw = 9 * g.cell_w();
    let ox = (x + w).saturating_sub(PAD() + bw);
    (ox.saturating_sub(PAD() + bw), ox, bw)
}

/// The top of version row `i`.
fn row_top(y: usize, rh: usize, i: usize) -> usize {
    y + 1 + (3 + i) * rh
}

/// QUARRYVERSIONS (B732): the button under source point `(sx, sy)`: `(path, seq, restore)`, or `None`.
pub(super) fn version_hit(g: &Geom, (sx, sy): (usize, usize)) -> Option<(String, u64, bool)> {
    let guard = PANEL.lock();
    let p = guard.as_ref()?;
    let v = p.versions.as_ref()?;
    let (x, y, w, h, rh) = frame(g, v.len());
    if sy < row_top(y, rh, 0) || sy >= y + h {
        return None;
    }
    let i = (sy - row_top(y, rh, 0)) / rh;
    let r = v.get(i)?;
    if (i + 4) * rh > h {
        return None; // clipped off the panel: not painted, not pressable
    }
    let (rx, ox, bw) = buttons(g, x, w);
    if sx >= rx && sx < rx + bw {
        Some((p.path.clone(), r.0, true))
    } else if sx >= ox && sx < ox + bw {
        Some((p.path.clone(), r.0, false))
    } else {
        None
    }
}

fn paint_versions(m: &Model, px: &mut [u32], p: &Panel, v: &[(u64, u64, u64)]) {
    let g = &m.geom;
    let (x, y, w, h, rh) = frame(g, v.len());
    fill(px, g, x, y, w, h, theme::surface_raised());
    keyline(px, g, Rect { x, y, w, h }, theme::accent());
    let clip = x + w;
    let ascii = |s: &str| -> Vec<u8> { s.bytes().map(|c| if (0x20..0x7f).contains(&c) { c } else { b'?' }).collect() };
    let line = |px: &mut [u32], i: usize, s: &str, ink: u32| {
        if (i + 1) * rh <= h {
            text(px, g, x + PAD(), y + 1 + i * rh + g.ts, &ascii(s), clip, ink);
        }
    };
    line(px, 0, &alloc::format!("Previous versions: {}", p.path), theme::text_primary());
    line(px, 1, &p.head, theme::text_secondary());
    if v.is_empty() {
        let s = match p.note {
            Some(why) => alloc::format!("No previous versions ({})", why),
            None => String::from("No previous versions"),
        };
        line(px, 3, &s, theme::text_primary());
        return;
    }
    line(px, 2, &alloc::format!("{} kept, newest first  (Restore keeps the current contents as a version)", v.len()), theme::text_secondary());
    let (rx, ox, bw) = buttons(g, x, w);
    for (i, r) in v.iter().enumerate() {
        if (i + 4) * rh > h {
            break;
        }
        let top = row_top(y, rh, i);
        let s = alloc::format!("{:<20} {:>10} bytes", super::ops::version_date(r.1), r.2);
        text(px, g, x + PAD(), top + g.ts, &ascii(&s), rx.min(clip), theme::text_primary());
        for (bx, label) in [(rx, "Restore"), (ox, "Open")] {
            keyline(px, g, Rect { x: bx, y: top, w: bw, h: rh }, theme::accent());
            text(px, g, bx + PAD(), top + g.ts, label.as_bytes(), (bx + bw).min(clip), theme::text_primary());
        }
    }
}
