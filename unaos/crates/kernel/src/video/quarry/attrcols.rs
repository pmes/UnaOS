// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Matrix — kernel-by-ruling R50
//!
//! ATTRCOLUMNS (rmbp-ledger B402) — Quarry's ATTRIBUTE COLUMNS, Tracker's inheritance (MACPARITY §16 B2).
//!
//! * **Add column…** — a right-click on the list HEADER opens a menu of the standard five and every attribute
//!   present in the folder (read from the listing's own inodes: the UnaFS index is keyed by a hash of the key
//!   name and cannot enumerate names). Choosing an attribute toggles its column; the folder's set is saved as
//!   the `una:view` attribute ON the folder (Be's `_trk`; FOLDERVIEW B6's seed), read back on the next visit.
//! * **Typed cells** — ints right-aligned, `media:duration_ms` as `m:ss`, `image:animated` yes/no, strings
//!   clipped. A header press on an attribute column sorts by it (folders first, empty cells last), a second
//!   press reverses.
//! * **Edit in place** — a press on an attribute cell of the SELECTED row opens the edit field (Return commits
//!   through `attrfacts::edit_in`, ONE transaction with its change time, as the session user — the ACL decides;
//!   Esc cancels; a double-click opens instead). `una:type` edits too: the opener follows the attribute.
//! * **Facts on the service pass** — a listed folder's files (and a file just opened) get their sniffed facts
//!   (`fs::attrfacts::refresh_in`) a few per pass, never in the click router and never at boot (R80); when a
//!   pass wrote something the listing is re-read so the new values show.
//!
//! Lock order: `MODEL` → `COLS` → [`ATTRS`]; the menu is a leaf. Design: `docs/dev/evidence/rmbp-1005/attrcolumns.md`.

use super::columns::{Cols, RowMeta};
use super::*;
use crate::fs::attrfacts as af;
use crate::fs::vfs::{AttrValue, MountTable, VfsError, KERNEL_PRINCIPAL};
use alloc::collections::VecDeque;
use core::cmp::Ordering as CmpOrd;
use core::sync::atomic::AtomicBool;

/// Rows whose attributes a listing reads (the folder's key set and the cells).
const SCAN_CAP: usize = 256;
/// Files whose facts one service pass refreshes.
const PER_PASS: usize = 4;

/// One cell's value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cell {
    None,
    Int(i64),
    Str(String),
}

impl Cell {
    fn of(v: &AttrValue) -> Cell {
        match v {
            AttrValue::Int(i) => Cell::Int(*i),
            AttrValue::Str(s) => Cell::Str(s.clone()),
            other => Cell::Str(af::fmt_value("", other)),
        }
    }
}

/// The folder's attribute view.
pub struct AttrState {
    pub dir: String,
    /// The chosen columns, in order (from `una:view`).
    pub keys: Vec<String>,
    /// Every non-bookkeeping key present in the folder's rows, with its type word.
    pub avail: Vec<(String, &'static str)>,
    /// Sorting by `keys[i]`.
    pub sort: Option<usize>,
    pending: VecDeque<String>,
    queued_dir: String,
    wrote: u32,
}

pub static ATTRS: crate::sync::Mutex<AttrState> = crate::sync::Mutex::new(AttrState {
    dir: String::new(),
    keys: Vec::new(),
    avail: Vec::new(),
    sort: None,
    pending: VecDeque::new(),
    queued_dir: String::new(),
    wrote: 0,
});

/// The header label for a key.
fn label(key: &str) -> String {
    match key {
        af::DURATION => String::from("DURATION"),
        af::ANIMATED => String::from("ANIM"),
        crate::fs::filetype::TYPE_KEY => String::from("MIME"),
        k => k.rsplit(':').next().unwrap_or(k).to_ascii_uppercase(),
    }
}

/// The column width (glyph columns) for a key.
fn width_of(key: &str, ty: Option<&str>) -> usize {
    match key {
        af::DURATION => 8,
        af::WIDTH | af::HEIGHT => 6,
        af::CODEC => 7,
        af::ANIMATED => 4,
        af::TITLE => 20,
        crate::fs::filetype::TYPE_KEY => 16,
        _ if ty == Some("int") => 8,
        _ => 14,
    }
}

/// Is `key` an integer column (the edit field parses it)?
fn int_key(key: &str) -> bool {
    matches!(key, af::DURATION | af::WIDTH | af::HEIGHT | af::ANIMATED)
}

pub fn widths() -> Vec<usize> {
    let st = ATTRS.lock();
    st.keys.iter().map(|k| width_of(k, st.avail.iter().find(|(a, _)| a == k).map(|(_, t)| *t))).collect()
}

pub fn sort_index() -> Option<usize> {
    ATTRS.lock().sort
}

pub fn set_sort(i: Option<usize>) {
    ATTRS.lock().sort = i;
}

pub fn key_at(i: usize) -> Option<String> {
    ATTRS.lock().keys.get(i).cloned()
}

/// The attribute column under header source-x `sx` (a column owns the gap before it). Pure.
pub(super) fn header_hit(c: &Cols, sx: usize) -> Option<usize> {
    if sx >= c.clip {
        return None;
    }
    let mut hit = None;
    for &(i, x, _) in c.attrs.iter() {
        if sx >= x.saturating_sub(c.cell_w) {
            hit = Some(i);
        }
    }
    hit
}

fn cmp_cell(a: &Cell, b: &Cell) -> CmpOrd {
    match (a, b) {
        (Cell::None, Cell::None) => CmpOrd::Equal,
        (Cell::None, _) => CmpOrd::Greater,
        (_, Cell::None) => CmpOrd::Less,
        (Cell::Int(x), Cell::Int(y)) => x.cmp(y),
        (Cell::Int(_), Cell::Str(_)) => CmpOrd::Less,
        (Cell::Str(_), Cell::Int(_)) => CmpOrd::Greater,
        (Cell::Str(x), Cell::Str(y)) => x.bytes().map(|c| c.to_ascii_lowercase()).cmp(y.bytes().map(|c| c.to_ascii_lowercase())),
    }
}

/// Sort `list` and its facts by attribute column `i`, STABLY; folders first, empty cells last in either
/// direction. A misaligned `meta` leaves the order alone. Pure.
pub fn sort_rows_by(list: &mut Vec<DirEnt>, meta: &mut Vec<RowMeta>, i: usize, desc: bool) {
    let aligned = meta.len() == list.len() && meta.iter().zip(list.iter()).all(|(m, e)| m.name == e.name);
    if !aligned {
        return;
    }
    let none = Cell::None;
    let mut idx: Vec<usize> = (0..list.len()).collect();
    idx.sort_by(|&a, &b| {
        let (ad, bd) = (matches!(list[a].kind, NodeKind::Dir), matches!(list[b].kind, NodeKind::Dir));
        let (ca, cb) = (meta[a].cells.get(i).unwrap_or(&none), meta[b].cells.get(i).unwrap_or(&none));
        let k = match (ca, cb) {
            (Cell::None, _) | (_, Cell::None) => cmp_cell(ca, cb),
            _ if desc => cmp_cell(ca, cb).reverse(),
            _ => cmp_cell(ca, cb),
        };
        bd.cmp(&ad).then(k)
    });
    let mut ls: Vec<Option<DirEnt>> = core::mem::take(list).into_iter().map(Some).collect();
    let mut ms: Vec<Option<RowMeta>> = core::mem::take(meta).into_iter().map(Some).collect();
    *list = idx.iter().filter_map(|&j| ls[j].take()).collect();
    *meta = idx.iter().filter_map(|&j| ms[j].take()).collect();
}

/// Read the rows' attributes (bounded): the folder's key set, and each row's cells for `keys`.
fn scan(mt: &MountTable, dir: &str, list: &[DirEnt], keys: &[String], meta: &mut [RowMeta]) -> Vec<(String, &'static str)> {
    let mut avail: Vec<(String, &'static str)> = Vec::new();
    for (n, e) in list.iter().enumerate().take(SCAN_CAP) {
        let attrs = mt.list_attrs(&join(dir, &e.name), KERNEL_PRINCIPAL).unwrap_or_default();
        for (k, v) in attrs.iter() {
            if !af::is_internal(k) && !avail.iter().any(|(a, _)| a == k) {
                avail.push((k.clone(), v.type_name()));
            }
        }
        if let Some(m) = meta.get_mut(n).filter(|m| m.name == e.name) {
            m.cells = keys.iter().map(|k| attrs.iter().find(|(a, _)| a == k).map(|(_, v)| Cell::of(v)).unwrap_or(Cell::None)).collect();
        }
    }
    avail.sort_by(|a, b| a.0.cmp(&b.0));
    avail
}

/// `columns::after_show`'s tail: a new folder loads its `una:view` and queues its files' facts; every listing
/// reads the key set and the cells. Nothing on a volume without attributes.
pub fn after_meta(mt: &MountTable, cwd: &str, list: &[DirEnt], meta: &mut Vec<RowMeta>) {
    let mut st = ATTRS.lock();
    if st.dir != cwd {
        st.dir = String::from(cwd);
        st.keys = af::view_of(mt, cwd);
        st.sort = None;
        st.avail.clear();
    }
    if matches!(mt.list_attrs(cwd, KERNEL_PRINCIPAL), Err(VfsError::Unsupported)) {
        st.keys.clear();
        st.avail.clear();
        return;
    }
    let keys = st.keys.clone();
    st.avail = scan(mt, cwd, list, &keys, meta);
    if st.queued_dir != cwd {
        st.queued_dir = String::from(cwd);
        st.pending.clear();
        for e in list.iter().filter(|e| matches!(e.kind, NodeKind::File)).take(SCAN_CAP) {
            st.pending.push_back(join(cwd, &e.name));
        }
    }
    if !keys.is_empty() {
        serial_println!("[attrcols] view dir={} cols={} avail={}", cwd, keys.join(","), st.avail.len());
    }
}

/// A file Quarry just opened: its facts first on the next service pass.
pub fn queue_open(path: &str) {
    ATTRS.lock().pending.push_front(String::from(path));
}

/// Quarry's service pass (chained from `columns::service`): refresh up to [`PER_PASS`] queued files' facts; when
/// the queue drains after a write, re-read the listing so the values show.
pub fn service() {
    let batch: Vec<String> = {
        let mut st = ATTRS.lock();
        let n = st.pending.len().min(PER_PASS);
        st.pending.drain(..n).collect()
    };
    if batch.is_empty() {
        return;
    }
    let mt = crate::shell::vfs_mount_table();
    let mut wrote = 0u32;
    for p in batch.iter() {
        if let af::Refresh::Wrote(n) = af::refresh_in(&mt, p, false) {
            wrote += 1;
            serial_println!("[attrfacts] path={} facts={} (ATTRCOLUMNS)", p, n);
        }
    }
    let (drained, total, dir) = {
        let mut st = ATTRS.lock();
        st.wrote += wrote;
        let t = st.wrote;
        if st.pending.is_empty() {
            st.wrote = 0;
        }
        (st.pending.is_empty(), t, st.dir.clone())
    };
    if drained && total > 0 {
        serial_println!("[attrcols] facts dir={} wrote={} -> relist", dir, total);
        relist();
    }
}

/// Re-read the listing (keeping the selected row) and repaint.
fn relist() {
    if let Some(m) = MODEL.lock().as_mut() {
        let sel = m.list.get(m.list_sel).map(|e| e.name.clone());
        let c = m.cwd.clone();
        m.invalidate();
        m.show(&c);
        if let Some(n) = sel {
            if let Some(i) = m.list.iter().position(|e| e.name == n) {
                m.list_sel = i;
            }
        }
        m.settle();
    }
    repaint();
}

fn cell_text(key: &str, c: &Cell, w: usize) -> Vec<u8> {
    let s = match c {
        Cell::None => return Vec::new(),
        Cell::Int(i) if key == af::DURATION => alloc::format!("{:>1$}", af::fmt_duration(*i), w),
        Cell::Int(i) if key == af::ANIMATED => String::from(if *i != 0 { "yes" } else { "no" }),
        Cell::Int(i) => alloc::format!("{:>1$}", i, w),
        Cell::Str(s) => s.clone(),
    };
    let mut v: Vec<u8> = s.bytes().map(|b| if (0x20..0x7f).contains(&b) { b } else { b'?' }).collect();
    v.truncate(w);
    v
}

/// The attribute columns' header labels (with the sort chevron).
pub(super) fn paint_header(px: &mut [u32], g: &Geom, c: &Cols, y: usize, chev: &str) {
    let st = ATTRS.lock();
    for (n, &(i, x, _)) in c.attrs.iter().enumerate() {
        let Some(k) = st.keys.get(i) else { continue };
        let mut s = label(k);
        if st.sort == Some(i) {
            s.push_str(chev);
        }
        let next = c.attrs.get(n + 1).map(|t| t.1).unwrap_or(c.clip).min(c.clip);
        text(px, g, x, y, s.as_bytes(), next, theme::title_text_inactive());
    }
}

/// One row's attribute cells.
pub(super) fn paint_cells(px: &mut [u32], g: &Geom, c: &Cols, meta: Option<&RowMeta>, y: usize, ink: u32) {
    let Some(m) = meta else { return };
    let st = ATTRS.lock();
    for (n, &(i, x, w)) in c.attrs.iter().enumerate() {
        let (Some(k), Some(cell)) = (st.keys.get(i), m.cells.get(i)) else { continue };
        let next = c.attrs.get(n + 1).map(|t| t.1).unwrap_or(c.clip).min(c.clip);
        text(px, g, x, y, &cell_text(k, cell, w), next, ink);
    }
}

// ── Add column… ─────────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
enum Item {
    Title,
    Std(&'static str),
    Attr(String, &'static str, bool),
    Empty,
}

struct Menu {
    x: usize,
    y: usize,
    items: Vec<Item>,
}

static MENU: crate::sync::Mutex<Option<Menu>> = crate::sync::Mutex::new(None);
static MENU_UP: AtomicBool = AtomicBool::new(false);

pub fn menu_up() -> bool {
    MENU_UP.load(Ordering::Acquire)
}

fn menu_w(g: &Geom) -> usize {
    34 * g.cell_w() + 2 * PAD()
}

/// Is source y `sy` on the list header?
pub(super) fn on_header(m: &Model, sx: usize, sy: usize) -> bool {
    let li = m.geom.list_pane().inner();
    sx >= li.x && sx < li.x + li.w && sy >= li.y && sy < li.y + m.geom.row_h()
}

/// Open the column menu at source `(sx, sy)` (the caller holds `MODEL`; this takes `ATTRS`, then the menu leaf).
pub(super) fn open_menu(m: &Model, sx: usize, sy: usize) {
    let g = &m.geom;
    let mut items = alloc::vec![Item::Title];
    for s in ["NAME", "SIZE", "MODIFIED", "TYPE", "ORIGIN"] {
        items.push(Item::Std(s));
    }
    {
        let st = ATTRS.lock();
        for (k, t) in st.avail.iter() {
            items.push(Item::Attr(k.clone(), *t, st.keys.iter().any(|c| c == k)));
        }
        if st.avail.is_empty() {
            items.push(Item::Empty);
        }
    }
    let (w, h) = (menu_w(g), items.len() * g.row_h() + 2);
    let n = items.len();
    *MENU.lock() = Some(Menu { x: sx.min(g.w.saturating_sub(w)), y: sy.min(g.h.saturating_sub(h)), items });
    MENU_UP.store(true, Ordering::Release);
    serial_println!("[attrcols] menu open items={} (Add column…)", n);
}

pub(super) fn paint_menu(m: &Model, px: &mut [u32]) {
    let g = &m.geom;
    let guard = MENU.lock();
    let Some(mn) = guard.as_ref() else { return };
    let (w, h) = (menu_w(g), mn.items.len() * g.row_h() + 2);
    fill(px, g, mn.x, mn.y, w, h, theme::button_face());
    keyline(px, g, Rect { x: mn.x, y: mn.y, w, h }, theme::frame_line());
    for (i, it) in mn.items.iter().enumerate() {
        let s = match it {
            Item::Title => String::from("Add column…"),
            Item::Std(s) => alloc::format!("  {} (standard)", s),
            Item::Attr(k, t, on) => alloc::format!("{} {} ({})", if *on { "[x]" } else { "[ ]" }, k, t),
            Item::Empty => String::from("  no attributes in this folder"),
        };
        let ink = if matches!(it, Item::Title | Item::Std(_) | Item::Empty) { theme::title_text_inactive() } else { theme::button_text() };
        let s = s.replace('…', "...");
        text(px, g, mn.x + PAD(), mn.y + 1 + i * g.row_h() + g.ts, s.as_bytes(), mn.x + w, ink);
    }
}

/// A primary press while the column menu is up (`hit` = Quarry source coordinates, or `None` outside): toggle
/// the attribute row under it, save the folder's view, re-read the listing. Always dismisses.
pub fn menu_press_at(hit: Option<(usize, usize)>) {
    let mn = MENU.lock().take();
    MENU_UP.store(false, Ordering::Release);
    let (Some(mn), Some((sx, sy))) = (mn, hit) else {
        repaint();
        return;
    };
    let (w, rh) = match MODEL.lock().as_ref() {
        Some(m) => (menu_w(&m.geom), m.geom.row_h()),
        None => return,
    };
    let picked = if sx >= mn.x && sx < mn.x + w && sy > mn.y { mn.items.get((sy - mn.y - 1) / rh).cloned() } else { None };
    let Some(Item::Attr(key, _, on)) = picked else {
        repaint();
        return;
    };
    toggle(&key, !on);
}

/// Add or remove `key`'s column in the current folder, save `una:view`, re-read the listing.
pub fn toggle(key: &str, add: bool) {
    let (dir, keys) = {
        let mut st = ATTRS.lock();
        if add {
            if !st.keys.iter().any(|k| k == key) {
                st.keys.push(String::from(key));
            }
        } else {
            st.keys.retain(|k| k != key);
        }
        st.sort = None;
        (st.dir.clone(), st.keys.clone())
    };
    let r = af::save_view(&crate::shell::vfs_mount_table(), &dir, &keys);
    serial_println!(
        "[attrcols] {} col={} dir={} cols={} view_saved={}",
        if add { "add" } else { "remove" },
        key,
        dir,
        if keys.is_empty() { String::from("-") } else { keys.join(",") },
        match &r { Ok(()) => String::from("1"), Err(e) => alloc::format!("0({:?})", e) }
    );
    relist();
}

// ── edit in place ───────────────────────────────────────────────────────────────────────────────

/// A press at source-x `sx` on row `i`, which was ALREADY selected: an attribute cell opens the edit field.
/// The caller holds `MODEL` (this takes `COLS`, `ATTRS`, then ops' edit leaf). `true` = consumed.
pub(super) fn press_cell(m: &Model, sx: usize, i: usize) -> bool {
    let g = m.geom;
    let li = g.list_pane().inner();
    let lsb = if m.list.len() > m.list_visible() { SBW() } else { 0 };
    let ws = widths();
    let (c, cell) = {
        let cs = super::columns::COLS.lock();
        let c = super::columns::layout_with(&g, li, lsb, &cs.widths, cs.trash, &ws);
        let Some(idx) = c.attrs.iter().rev().find(|t| sx >= t.1 && sx < t.1 + (t.2 + 1) * c.cell_w).map(|t| t.0) else { return false };
        let cell = cs.meta.get(i).filter(|mm| m.list.get(i).map_or(false, |e| e.name == mm.name)).and_then(|mm| mm.cells.get(idx).cloned());
        (idx, cell)
    };
    let Some(key) = key_at(c) else { return false };
    let Some(e) = m.list.get(i) else { return false };
    let int = matches!(cell, Some(Cell::Int(_))) || int_key(&key);
    let buf = match cell {
        Some(Cell::Int(v)) => alloc::format!("{}", v),
        Some(Cell::Str(s)) => s,
        _ => String::new(),
    };
    let path = join(&m.cwd, &e.name);
    serial_println!("[attrcols] edit begin path={} key={} int={}", path, key, int as u8);
    super::ops::begin_attr_edit(path, key, buf, int);
    true
}

/// The edit field's Return: parse, write (ONE transaction with the change time, as `principal`), re-read.
pub fn commit(path: &str, key: &str, buf: &str, int: bool, principal: &str) -> Result<(), String> {
    let v = if int {
        AttrValue::Int(buf.trim().parse::<i64>().map_err(|_| String::from("not-a-number"))?)
    } else {
        AttrValue::Str(String::from(buf.trim()))
    };
    let r = af::edit_in(&crate::shell::vfs_mount_table(), path, key, v, principal).map_err(|e| alloc::format!("{:?}", e));
    serial_println!("[attrcols] edit path={} key={} ok={} reason={}", path, key, r.is_ok() as u8, r.as_ref().err().map(|s| s.as_str()).unwrap_or("-"));
    relist();
    r
}

// ── the fixture (`tests attrcolumns` leg 2) ─────────────────────────────────────────────────────

/// Over `dir` (test-f): the folder offers `media:duration_ms` and `doc:title`; both columns fill from the
/// attributes, fit the logical bench panel after the standard ones, and the duration sort is ascending.
/// Returns the columns that rendered values. Leaves the live view untouched.
pub fn selftest_columns(mt: &MountTable, dir: &str) -> Result<usize, String> {
    let list: Vec<DirEnt> = mt.read_dir(dir).map_err(|e| alloc::format!("read_dir {:?}", e))?.into_iter().filter(|e| e.name != "." && e.name != "..").collect();
    let keys = [String::from(af::DURATION), String::from(af::TITLE)];
    let mut meta = super::columns::compute_meta(mt, dir, &list, &[]);
    let avail = scan(mt, dir, &list, &keys, &mut meta);
    for k in keys.iter() {
        if !avail.iter().any(|(a, _)| a == k) {
            return Err(alloc::format!("not-offered({})", k));
        }
    }
    let added = (0..keys.len()).filter(|&i| meta.iter().any(|m| m.cells.get(i).map_or(false, |c| *c != Cell::None))).count();
    let mut l2 = list.clone();
    sort_rows_by(&mut l2, &mut meta, 0, false);
    let durs: Vec<i64> = meta.iter().filter_map(|m| if let Some(Cell::Int(d)) = m.cells.first() { Some(*d) } else { None }).collect();
    if durs.windows(2).any(|w| w[0] > w[1]) || durs.is_empty() {
        return Err(String::from("duration-sort"));
    }
    let shown = cell_text(af::DURATION, &Cell::Int(durs[0]), 8);
    let big = super::geometry(crate::ui::px(1920), crate::ui::px(1200)).ok_or("no-geometry")?;
    let ws: Vec<usize> = keys.iter().map(|k| width_of(k, None)).collect();
    let c = super::columns::layout_with(&big, big.list_pane().inner(), 0, &super::columns::DEFAULT_WIDTHS, false, &ws);
    serial_println!(
        "[attrcols] fixture dir={} avail={} cols={} filled={} fit={} first={}({}) shortest={}",
        dir,
        avail.len(),
        keys.join(","),
        added,
        c.attrs.len(),
        l2.first().map(|e| e.name.as_str()).unwrap_or("-"),
        core::str::from_utf8(&shown).unwrap_or("?").trim(),
        durs[0]
    );
    if c.attrs.len() != keys.len() {
        return Err(alloc::format!("fit={}", c.attrs.len()));
    }
    Ok(added)
}
