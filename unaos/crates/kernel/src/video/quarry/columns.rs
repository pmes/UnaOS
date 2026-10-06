// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Matrix — kernel-by-ruling R50
//!
//! QUARRY2 (rmbp-ledger B336) — the list view's COLUMNS and its SORT. Quarry is the Finder by ruling
//! (R50); what a Finder's list shows about a file is the Finder's business, and it is done here.
//!
//! * **TYPE** — the file's type, by its short name (`una:name` of `/system/types/<mime>`, else the
//!   builtin association row: `Markdown`, `PNG image`). The type is `fs::filetype`'s: the `una:type`
//!   attribute on a volume that takes attributes (UnaFS), else the one extension table, else the sniff
//!   (only for a name the table does not know, at most [`SNIFF_CAP`] per listing, so a navigation never
//!   reads every file of a big directory). No store of its own (R79).
//! * **ORIGIN** — in the session user's Trash only: where the item was trashed from, read from
//!   `fs::trash::entries()` (TRASHTIME's `una:trash-origin` attribute on UnaFS, the `.index` on FAT).
//! * **The widths and the sort are a preference** — Principia's store through the kernel prefs path
//!   (`prefs::get/set`, namespace [`NS`]). A change is LATCHED and written on Quarry's service pass
//!   ([`service`]), never in the click router: a save is a file write.
//! * **The sort** — a header click on NAME / SIZE / MODIFIED / TYPE sorts by that column, a second
//!   click on the same column reverses it, and the header carries a chevron (`^` ascending, `v`
//!   descending). Folders stay first in either direction. The sort is STABLE (`slice::sort_by`, a merge
//!   sort): rows whose keys tie keep the order they had.
//!
//! The per-row facts live in [`COLS`] beside the model (lock order: `MODEL` then `COLS`, everywhere),
//! each tagged with the row's name so a list another path replaced (the fixtures assign `m.list`
//! directly) paints a dash rather than another file's type.
//!
//! Keys (list focus): `s` cycles the sort column, `[` / `]` narrow / widen the sorted column (TYPE when
//! sorting by name, which has no width of its own).
//!
//! Witness: `:: QUARRY2: columns=type,origin sort=name|size|mtime|type types=+markdown,+json
//! gif_frames=<n|skip> -> PASS ::` (`tests quarry2`, [`selftest`]).
//!
//! Design: `docs/dev/evidence/rmbp-1005/QUARRY2.md`.

use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering as CmpOrd;
use core::sync::atomic::{AtomicBool, Ordering};

use super::{fill, join, mtime_field, size_field, text, theme, DirEnt, Geom, Model, NodeKind, Rect, VfsTime, MODEL, PAD};
use crate::fs::vfs::{AttrValue, MountTable, VfsError, KERNEL_PRINCIPAL};

/// The preference namespace.
pub const NS: &str = "quarry";
/// Files the listing may sniff per directory read (names the extension table does not know).
pub const SNIFF_CAP: usize = 32;
/// Width bounds, in glyph columns.
const W_MIN: usize = 4;
const W_MAX: usize = 64;
/// The name column never shrinks below these to make room (size/modified, then the wide columns).
const NAME_FLOOR_SIZE: usize = 12;
const NAME_FLOOR_MOD: usize = 7;
const NAME_FLOOR_WIDE: usize = 16;

/// A list column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Col {
    Name,
    Size,
    Modified,
    Type,
    Origin,
}

impl Col {
    fn label(self) -> &'static str {
        match self {
            Col::Name => "NAME",
            Col::Size => "SIZE",
            Col::Modified => "MODIFIED",
            Col::Type => "TYPE",
            Col::Origin => "ORIGIN",
        }
    }
    fn pref_key(self) -> &'static str {
        match self {
            Col::Name => "col.name",
            Col::Size => "col.size",
            Col::Modified => "col.modified",
            Col::Type => "col.type",
            Col::Origin => "col.origin",
        }
    }
    fn sort_key(self) -> Option<SortKey> {
        match self {
            Col::Name => Some(SortKey::Name),
            Col::Size => Some(SortKey::Size),
            Col::Modified => Some(SortKey::Mtime),
            Col::Type => Some(SortKey::Type),
            Col::Origin => None,
        }
    }
}

/// What the list is sorted by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Mtime,
    Type,
}

impl SortKey {
    pub const ALL: [SortKey; 4] = [SortKey::Name, SortKey::Size, SortKey::Mtime, SortKey::Type];
    pub fn name(self) -> &'static str {
        match self {
            SortKey::Name => "name",
            SortKey::Size => "size",
            SortKey::Mtime => "mtime",
            SortKey::Type => "type",
        }
    }
    fn parse(s: &str) -> Option<SortKey> {
        SortKey::ALL.iter().copied().find(|k| k.name() == s)
    }
    fn col(self) -> Col {
        match self {
            SortKey::Name => Col::Name,
            SortKey::Size => Col::Size,
            SortKey::Mtime => Col::Modified,
            SortKey::Type => Col::Type,
        }
    }
    fn next(self) -> SortKey {
        match self {
            SortKey::Name => SortKey::Size,
            SortKey::Size => SortKey::Mtime,
            SortKey::Mtime => SortKey::Type,
            SortKey::Type => SortKey::Name,
        }
    }
}

/// Column widths in glyph columns (the name column takes what is left).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Widths {
    pub size: usize,
    pub modified: usize,
    pub ty: usize,
    pub origin: usize,
}

pub const DEFAULT_WIDTHS: Widths = Widths { size: 9, modified: 16, ty: 12, origin: 28 };

impl Widths {
    fn of(&self, c: Col) -> usize {
        match c {
            Col::Name => 0,
            Col::Size => self.size,
            Col::Modified => self.modified,
            Col::Type => self.ty,
            Col::Origin => self.origin,
        }
    }
    fn set(&mut self, c: Col, w: usize) {
        let w = w.clamp(W_MIN, W_MAX);
        match c {
            Col::Name => {}
            Col::Size => self.size = w,
            Col::Modified => self.modified = w,
            Col::Type => self.ty = w,
            Col::Origin => self.origin = w,
        }
    }
}

/// One row's derived facts, tagged with the row's name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowMeta {
    pub name: String,
    pub mime: String,
    pub type_name: String,
    pub origin: Option<String>,
}

/// The column state beside the model.
pub struct ColState {
    pub meta: Vec<RowMeta>,
    pub key: SortKey,
    pub desc: bool,
    pub widths: Widths,
    pub trash: bool,
    loaded: bool,
}

pub static COLS: spin::Mutex<ColState> = spin::Mutex::new(ColState {
    meta: Vec::new(),
    key: SortKey::Name,
    desc: false,
    widths: DEFAULT_WIDTHS,
    trash: false,
    loaded: false,
});

/// A change to persist on the next service pass.
static PERSIST: AtomicBool = AtomicBool::new(false);

/// Read the widths and the sort from the preference store once per boot (`prefs::get` does no I/O).
fn load(st: &mut ColState) {
    if st.loaded {
        return;
    }
    st.loaded = true;
    for c in [Col::Size, Col::Modified, Col::Type, Col::Origin] {
        if let Some(w) = crate::prefs::get(NS, c.pref_key()).and_then(|v| v.as_int()) {
            st.widths.set(c, w.max(0) as usize);
        }
    }
    if let Some(k) = crate::prefs::get(NS, "sort.key").and_then(|v| v.as_str().and_then(SortKey::parse)) {
        st.key = k;
    }
    if let Some(d) = crate::prefs::get(NS, "sort.desc").and_then(|v| v.as_bool()) {
        st.desc = d;
    }
}

/// Write the widths and the sort (only changed keys are re-saved by `prefs::set`). Quarry's service
/// pass — chained from `live::service`, never from a router.
pub fn service() {
    if !PERSIST.swap(false, Ordering::AcqRel) {
        return;
    }
    let (w, key, desc) = {
        let st = COLS.lock();
        (st.widths, st.key, st.desc)
    };
    use crate::prefs::PrefValue;
    let mut ok = 0usize;
    for c in [Col::Size, Col::Modified, Col::Type, Col::Origin] {
        ok += crate::prefs::set(NS, c.pref_key(), PrefValue::Int(w.of(c) as i64)).is_ok() as usize;
    }
    ok += crate::prefs::set(NS, "sort.key", PrefValue::Str(String::from(key.name()))).is_ok() as usize;
    ok += crate::prefs::set(NS, "sort.desc", PrefValue::Bool(desc)).is_ok() as usize;
    serial_println!(
        "[quarry2] prefs ns={} size={} modified={} type={} origin={} sort={} desc={} ok={}/6",
        NS, w.size, w.modified, w.ty, w.origin, key.name(), desc as u8, ok
    );
}

// ── Layout ──────────────────────────────────────────────────────────────────────────────────────

/// Where each visible column sits. `cols` is in display order (NAME first, implicitly).
pub(super) struct Cols {
    pub name_x: usize,
    pub name_cols: usize,
    /// `(column, x, width in glyph columns)`.
    pub cols: Vec<(Col, usize, usize)>,
    pub clip: usize,
    pub cell_w: usize,
}

/// The ONE column layout: the painter and the header press both read it. Columns degrade rather
/// than overlap — SIZE, then MODIFIED (the pre-QUARRY2 thresholds exactly), then TYPE only while the name
/// keeps [`NAME_FLOOR_WIDE`] columns; the Trash takes ORIGIN and TYPE first (SMALLFIX2). Pure.
pub(super) fn layout(g: &Geom, li: Rect, lsb: usize, w: &Widths, trash: bool) -> Cols {
    let cell_w = g.cell_w().max(1);
    let total = li.w.saturating_sub(lsb).saturating_sub(2 * PAD()) / cell_w;
    // SMALLFIX2 (B391): the Trash ranks its OWN column first — ORIGIN, then TYPE, then SIZE, MODIFIED — so a panel one
    // column short of all four (the rMBP at 2.5: the chrome cell rounds 22.5 up to 23) drops MODIFIED, never ORIGIN.
    let prio: &[Col] = if trash { &[Col::Origin, Col::Type, Col::Size, Col::Modified] } else { &[Col::Size, Col::Modified, Col::Type] };
    let mut chosen = [false; 5];
    let mut used = 0usize;
    for &c in prio {
        let floor = match c {
            Col::Size => NAME_FLOOR_SIZE,
            Col::Modified => NAME_FLOOR_MOD,
            _ => NAME_FLOOR_WIDE,
        };
        let cost = w.of(c) + 1;
        if total >= used + cost + floor {
            used += cost;
            chosen[c as usize] = true;
        }
    }
    let name_cols = if used == 0 { total.saturating_sub(2) } else { total.saturating_sub(used) };
    let name_x = li.x + PAD();
    let mut x = name_x + (name_cols + 1) * cell_w;
    let mut cols = Vec::new();
    for c in [Col::Size, Col::Modified, Col::Type, Col::Origin] {
        if chosen[c as usize] {
            cols.push((c, x, w.of(c)));
            x += (w.of(c) + 1) * cell_w;
        }
    }
    Cols { name_x, name_cols, cols, clip: li.x + li.w - lsb, cell_w }
}

/// The column under source x `sx` in the header (a column owns its gap to the next one). Pure.
pub(super) fn header_hit(c: &Cols, sx: usize) -> Option<Col> {
    if sx < c.name_x || sx >= c.clip {
        return None;
    }
    let mut hit = Col::Name;
    for &(col, x, _) in c.cols.iter() {
        if sx >= x.saturating_sub(c.cell_w) {
            hit = col;
        }
    }
    Some(hit)
}

// ── The per-row facts ───────────────────────────────────────────────────────────────────────────

/// The listing's type for `path` (`name` its leaf): the attribute when `attrs`, the table, the sniff
/// while `sniffs` lasts, else unknown. Returns the MIME string.
fn listing_mime(mt: &MountTable, path: &str, e: &DirEnt, attrs: bool, sniffs: &mut usize) -> String {
    use crate::fs::filetype as ft;
    if matches!(e.kind, NodeKind::Dir) {
        return String::from(ft::DIRECTORY);
    }
    if attrs {
        if let Ok(AttrValue::Str(m)) = mt.get_attr(path, ft::TYPE_KEY, KERNEL_PRINCIPAL) {
            if !m.is_empty() {
                return m;
            }
        }
    }
    if let Some(m) = ft::by_extension(&e.name) {
        return String::from(m);
    }
    if e.size > 0 && *sniffs < SNIFF_CAP {
        *sniffs += 1;
        let want = core::cmp::min(e.size, ft::SNIFF_LEN as u64) as usize;
        if let Ok(head) = mt.read(path, 0, want) {
            if let Some(m) = ft::sniff(&head) {
                return String::from(m);
            }
        }
    }
    String::from(ft::OCTET)
}

/// Is `cwd` the session user's Trash?
pub fn is_trash(cwd: &str) -> bool {
    cwd.eq_ignore_ascii_case(&crate::fs::trash::trash_dir())
}

/// Compute the facts for `list` in directory `cwd`. `origins` = the Trash's `(name, origin)` pairs
/// when the directory is the Trash (empty otherwise).
pub fn compute_meta(mt: &MountTable, cwd: &str, list: &[DirEnt], origins: &[(String, String)]) -> Vec<RowMeta> {
    let attrs = !matches!(mt.list_attrs(cwd, KERNEL_PRINCIPAL), Err(VfsError::Unsupported));
    let mut sniffs = 0usize;
    let mut names: Vec<(String, String)> = Vec::new();
    let mut out = Vec::with_capacity(list.len());
    for e in list {
        let path = join(cwd, &e.name);
        let mime = listing_mime(mt, &path, e, attrs, &mut sniffs);
        let type_name = match names.iter().find(|(m, _)| *m == mime) {
            Some((_, n)) => n.clone(),
            None => {
                let n = crate::fs::assoc::name_of_in(mt, &mime);
                names.push((mime.clone(), n.clone()));
                n
            }
        };
        let origin = origins.iter().find(|(n, _)| n.eq_ignore_ascii_case(&e.name)).map(|(_, o)| o.clone());
        out.push(RowMeta { name: e.name.clone(), mime, type_name, origin });
    }
    out
}

fn time_key(t: Option<&VfsTime>) -> (u16, u8, u8, u8, u8, u8) {
    t.map(|t| (t.year, t.month, t.day, t.hour, t.min, t.sec)).unwrap_or((0, 0, 0, 0, 0, 0))
}

/// Order two rows by `key` (folders first in either direction; `desc` reverses the key only). Pure.
pub fn cmp_rows(a: &DirEnt, am: Option<&RowMeta>, b: &DirEnt, bm: Option<&RowMeta>, key: SortKey, desc: bool) -> CmpOrd {
    let ad = matches!(a.kind, NodeKind::Dir);
    let bd = matches!(b.kind, NodeKind::Dir);
    let k = match key {
        SortKey::Name => a.name.cmp(&b.name),
        SortKey::Size => a.size.cmp(&b.size),
        SortKey::Mtime => time_key(a.mtime.as_ref()).cmp(&time_key(b.mtime.as_ref())),
        SortKey::Type => {
            let at = am.map(|m| m.type_name.as_str()).unwrap_or("");
            let bt = bm.map(|m| m.type_name.as_str()).unwrap_or("");
            let mut ai = at.bytes().map(|c| c.to_ascii_lowercase());
            let mut bi = bt.bytes().map(|c| c.to_ascii_lowercase());
            loop {
                match (ai.next(), bi.next()) {
                    (None, None) => break CmpOrd::Equal,
                    (None, Some(_)) => break CmpOrd::Less,
                    (Some(_), None) => break CmpOrd::Greater,
                    (Some(x), Some(y)) if x != y => break x.cmp(&y),
                    _ => {}
                }
            }
        }
    };
    bd.cmp(&ad).then(if desc { k.reverse() } else { k })
}

/// Sort `list` and its facts together, STABLY (`sort_by` is a merge sort: equal keys keep their input
/// order). `meta` may be shorter or misaligned (then the type key reads as empty). Pure.
pub fn sort_rows(list: &mut Vec<DirEnt>, meta: &mut Vec<RowMeta>, key: SortKey, desc: bool) {
    let aligned = meta.len() == list.len() && meta.iter().zip(list.iter()).all(|(m, e)| m.name == e.name);
    let mut idx: Vec<usize> = (0..list.len()).collect();
    idx.sort_by(|&i, &j| {
        let (mi, mj) = if aligned { (meta.get(i), meta.get(j)) } else { (None, None) };
        cmp_rows(&list[i], mi, &list[j], mj, key, desc)
    });
    let old_l = core::mem::take(list);
    let mut slots: Vec<Option<DirEnt>> = old_l.into_iter().map(Some).collect();
    *list = idx.iter().filter_map(|&i| slots[i].take()).collect();
    if aligned {
        let old_m = core::mem::take(meta);
        let mut ms: Vec<Option<RowMeta>> = old_m.into_iter().map(Some).collect();
        *meta = idx.iter().filter_map(|&i| ms[i].take()).collect();
    }
}

/// Re-sort the model's list by the current key, keeping the selected ROW selected.
fn resort(m: &mut Model, st: &mut ColState) {
    let sel = m.list.get(m.list_sel).map(|e| e.name.clone());
    sort_rows(&mut m.list, &mut st.meta, st.key, st.desc);
    if let Some(n) = sel {
        if let Some(i) = m.list.iter().position(|e| e.name == n) {
            m.list_sel = i;
        }
    }
}

/// `Model::show`'s tail: derive the facts for the new listing and apply the sort. Inside the model
/// lock (the listing itself was read there too).
pub(super) fn after_show(m: &mut Model) {
    let mut st = COLS.lock();
    load(&mut st);
    st.trash = is_trash(&m.cwd);
    if m.list.is_empty() {
        st.meta.clear();
        return;
    }
    let mt = crate::shell::vfs_mount_table();
    let origins: Vec<(String, String)> = if st.trash {
        crate::fs::trash::entries().into_iter().map(|e| (e.name, e.orig)).collect()
    } else {
        Vec::new()
    };
    st.meta = compute_meta(&mt, &m.cwd, &m.list, &origins);
    let st = &mut *st;
    resort(m, st);
    let typed = st.meta.iter().filter(|r| r.mime != crate::fs::filetype::OCTET).count();
    serial_println!(
        "[quarry2] list cwd={} rows={} typed={} trash={} origins={} sort={} desc={}",
        m.cwd, m.list.len(), typed, st.trash as u8, origins.len(), st.key.name(), st.desc as u8
    );
}

/// A press on the list HEADER (`sx` in source pixels). Sorts by the column under it; a second press on
/// the sorted column reverses. Returns whether the model changed.
pub(super) fn header_press(m: &mut Model, sx: usize) -> bool {
    let g = m.geom;
    let li = g.list_pane().inner();
    let lsb = if m.list.len() > m.list_visible() { super::SBW() } else { 0 };
    let mut st = COLS.lock();
    load(&mut st);
    let c = layout(&g, li, lsb, &st.widths, st.trash);
    let Some(col) = header_hit(&c, sx) else { return false };
    let Some(key) = col.sort_key() else {
        serial_println!("[quarry2] header press col={} -> not sortable", col.label());
        return false;
    };
    if st.key == key {
        st.desc = !st.desc;
    } else {
        st.key = key;
        st.desc = false;
    }
    let st = &mut *st;
    resort(m, st);
    PERSIST.store(true, Ordering::Release);
    serial_println!("[quarry2] header press col={} sort={} desc={}", col.label(), st.key.name(), st.desc as u8);
    true
}

/// List-focus keys: `s` cycles the sort column, `[` / `]` narrow / widen the sorted column. `true`
/// when consumed (the caller repaints).
pub(super) fn key(c: u8) -> bool {
    if !matches!(c, b's' | b'[' | b']') {
        return false;
    }
    let mut guard = MODEL.lock();
    let Some(m) = guard.as_mut() else { return false };
    if m.focus != super::Pane::List || m.err.is_some() {
        return false;
    }
    let mut st = COLS.lock();
    load(&mut st);
    match c {
        b's' => {
            st.key = st.key.next();
            st.desc = false;
            let st = &mut *st;
            resort(m, st);
        }
        _ => {
            let col = if st.key == SortKey::Name { Col::Type } else { st.key.col() };
            let w = st.widths.of(col);
            let nw = if c == b'[' { w.saturating_sub(1) } else { w + 1 };
            st.widths.set(col, nw);
        }
    }
    PERSIST.store(true, Ordering::Release);
    serial_println!(
        "[quarry2] key={} sort={} desc={} widths={},{},{},{}",
        c as char, st.key.name(), st.desc as u8, st.widths.size, st.widths.modified, st.widths.ty, st.widths.origin
    );
    m.settle();
    true
}

// ── The painter ─────────────────────────────────────────────────────────────────────────────────

fn clipped(s: &str, n: usize) -> Vec<u8> {
    let mut v: Vec<u8> = s.bytes().map(|b| if (0x20..0x7f).contains(&b) { b } else { b'?' }).collect();
    v.truncate(n);
    v
}

/// The list pane's header and body (the pane fill, keyline and scrollbar stay `repaint_locked`'s).
/// Returns the body's top y.
pub(super) fn paint_list(m: &Model, px: &mut [u32], li: Rect, lsb: usize, lvis: usize) -> usize {
    let g = &m.geom;
    let row_h = g.row_h();
    let st = COLS.lock();
    let c = layout(g, li, lsb, &st.widths, st.trash);
    let clip = c.clip;
    let next_x = |i: usize| c.cols.get(i + 1).map(|t| t.1).unwrap_or(clip).min(clip);
    let size_x = c.cols.first().map(|t| t.1).unwrap_or(clip);

    // Header — outside the scrolled band by construction, so a scrolled list never loses its columns.
    fill(px, g, li.x, li.y, li.w, row_h, theme::CHROME_FACE);
    fill(px, g, li.x, li.y + row_h - 1, li.w, 1, theme::FRAME_LINE);
    let chev = if st.desc { " v" } else { " ^" };
    let label = |col: Col| -> Vec<u8> {
        let mut s = Vec::from(col.label().as_bytes());
        if col.sort_key() == Some(st.key) {
            s.extend_from_slice(chev.as_bytes());
        }
        s
    };
    text(px, g, c.name_x, li.y + g.ts, &label(Col::Name), size_x.min(clip), theme::TITLE_TEXT_INACTIVE);
    for (i, &(col, x, _)) in c.cols.iter().enumerate() {
        text(px, g, x, li.y + g.ts, &label(col), next_x(i), theme::TITLE_TEXT_INACTIVE);
    }
    let body_y = li.y + row_h;
    if let Some(e) = &m.err {
        text(px, g, c.name_x, body_y + g.ts, e.as_bytes(), clip, theme::CONTROL_CLOSE);
        return body_y;
    }
    for r in 0..lvis {
        let i = m.list_scroll + r;
        if i >= m.list.len() {
            break;
        }
        let ent = &m.list[i];
        let meta = st.meta.get(i).filter(|mm| mm.name == ent.name);
        let y = body_y + r * row_h;
        let sel = i == m.list_sel;
        if sel {
            let col = if m.focus == super::Pane::List { theme::ACCENT } else { theme::SCROLL_THUMB };
            fill(px, g, li.x, y, li.w - lsb, row_h, col);
        }
        let ink = if sel && m.focus == super::Pane::List { theme::CHROME_FACE } else { theme::CONTENT_TEXT };
        let dir = matches!(ent.kind, NodeKind::Dir);
        let mut nm: Vec<u8> = Vec::new();
        nm.extend_from_slice(ent.name.as_bytes());
        // `ls -F`'s two marks: `/` descends, `*` RUNS (see the pre-QUARRY2 painter's note).
        if dir {
            nm.push(b'/');
        } else if super::is_executable(&ent.name) {
            nm.push(b'*');
        }
        nm.truncate(c.name_cols);
        text(px, g, c.name_x, y + g.ts, &nm, size_x.min(clip), ink);
        for (k, &(col, x, w)) in c.cols.iter().enumerate() {
            let cell: Vec<u8> = match col {
                Col::Size => {
                    if dir {
                        alloc::format!("{:>1$}", "--", w).into_bytes()
                    } else {
                        size_field(ent.size, w).into_bytes()
                    }
                }
                Col::Modified => mtime_field(ent.mtime.as_ref()).into_bytes(),
                Col::Type => clipped(meta.map(|mm| mm.type_name.as_str()).unwrap_or("-"), w),
                Col::Origin => clipped(meta.and_then(|mm| mm.origin.as_deref()).unwrap_or("-"), w),
                Col::Name => Vec::new(),
            };
            text(px, g, x, y + g.ts, &cell, next_x(k), ink);
        }
    }
    body_y
}

// ── The fixture ─────────────────────────────────────────────────────────────────────────────────

/// `tests quarry2` registration, once.
pub fn ensure_tests() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("quarry2", selftest);
        // FACETANIM (B358): `tests facetanim` rides this registration (no tests.rs line).
        #[cfg(feature = "facet")]
        crate::tests::register("facetanim", crate::video::facet::anim::selftest);
    }
}

fn ent(name: &str, dir: bool, size: u64, min: u8) -> DirEnt {
    DirEnt {
        name: String::from(name),
        kind: if dir { NodeKind::Dir } else { NodeKind::File },
        size,
        mtime: Some(VfsTime { year: 2026, month: 10, day: 4, hour: 12, min, sec: 0 }),
    }
}

fn meta_of(list: &[DirEnt], ty: &[&str]) -> Vec<RowMeta> {
    list.iter()
        .zip(ty.iter())
        .map(|(e, t)| RowMeta { name: e.name.clone(), mime: String::new(), type_name: String::from(*t), origin: None })
        .collect()
}

fn names(list: &[DirEnt]) -> String {
    let mut s = String::new();
    for (i, e) in list.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&e.name);
    }
    s
}

/// Leg: the column layout carries TYPE everywhere and ORIGIN in the Trash on the bench panel, the
/// header press maps back onto the painter's columns, and the narrow panel degrades. `Err` names it.
fn leg_columns() -> Result<&'static str, String> {
    // SMALLFIX2 (B391): the fixture is the bench panel in LOGICAL px. Since UIMETRICS Quarry is physical px at the dpi
    // scale, and KFONTPPI (merge16) re-latched the rMBP from 1.0 to 2.5 — so a physical 1920x1200 fixture was a
    // 768x480-logical panel on which TYPE correctly degrades (flights 24/25: `plain list cols=[Size, Modified]`).
    let (bw, bh) = (crate::ui::px(1920), crate::ui::px(1200));
    let big = super::geometry(bw, bh).ok_or("no geometry for the logical 1920x1200 bench panel")?;
    let li = big.list_pane().inner();
    let c = layout(&big, li, 0, &DEFAULT_WIDTHS, false);
    let has = |c: &Cols, k: Col| c.cols.iter().any(|t| t.0 == k);
    if !has(&c, Col::Type) || has(&c, Col::Origin) {
        return Err(alloc::format!("plain list cols={:?}", c.cols.iter().map(|t| t.0).collect::<Vec<_>>()));
    }
    let t = layout(&big, li, 0, &DEFAULT_WIDTHS, true);
    let names_of = |c: &Cols| c.cols.iter().map(|t| t.0.label()).collect::<Vec<_>>().join("+");
    let s2 = crate::video::dpi::s2();
    serial_println!("[quarry2] fixture panel={}x{} scale={}.{} plain={} trash={}", bw, bh, s2 / 2, if s2 % 2 == 1 { 5 } else { 0 }, names_of(&c), names_of(&t));
    if !has(&t, Col::Type) || !has(&t, Col::Origin) {
        return Err(alloc::format!("trash list cols={:?} name_cols={}", t.cols.iter().map(|t| t.0).collect::<Vec<_>>(), t.name_cols));
    }
    for &(col, x, _) in t.cols.iter() {
        if header_hit(&t, x + 1) != Some(col) {
            return Err(alloc::format!("header hit at x={} is not {:?}", x + 1, col));
        }
    }
    if header_hit(&t, t.name_x + 1) != Some(Col::Name) {
        return Err(String::from("header hit on NAME"));
    }
    let small = super::geometry(crate::ui::px(640), crate::ui::px(480)).ok_or("no geometry for the logical 640x480 panel")?; // SMALLFIX2: logical, as above
    let s = layout(&small, small.list_pane().inner(), 0, &DEFAULT_WIDTHS, true);
    if s.name_cols < NAME_FLOOR_MOD.min(NAME_FLOOR_SIZE) && !s.cols.is_empty() {
        return Err(alloc::format!("small panel squeezed the name to {}", s.name_cols));
    }
    Ok("type,origin")
}

/// Leg: every sort key orders correctly, folders stay first, desc reverses, and ties keep their order.
fn leg_sort() -> Result<&'static str, String> {
    let base = alloc::vec![
        ent("b.txt", false, 300, 3),
        ent("DIR", true, 0, 9),
        ent("a.png", false, 100, 5),
        ent("c.md", false, 100, 1),
    ];
    let ty = ["Plain text", "Folder", "PNG image", "Markdown"];
    let cases: [(SortKey, bool, &str); 5] = [
        (SortKey::Name, false, "DIR,a.png,b.txt,c.md"),
        (SortKey::Size, false, "DIR,a.png,c.md,b.txt"), // a.png and c.md tie at 100: input order kept
        (SortKey::Mtime, false, "DIR,c.md,b.txt,a.png"),
        (SortKey::Type, false, "DIR,c.md,b.txt,a.png"),
        (SortKey::Size, true, "DIR,b.txt,a.png,c.md"), // desc: tie still in input order
    ];
    for (key, desc, want) in cases.iter() {
        let mut l = base.clone();
        let mut mm = meta_of(&l, &ty);
        sort_rows(&mut l, &mut mm, *key, *desc);
        let got = names(&l);
        if got != *want {
            return Err(alloc::format!("sort={} desc={} got {} want {}", key.name(), desc, got, want));
        }
        if mm.iter().zip(l.iter()).any(|(m, e)| m.name != e.name) {
            return Err(alloc::format!("sort={} left the facts misaligned", key.name()));
        }
    }
    // Stability, from the other side: ties under `size` keep a REVERSED input order too.
    let mut l = alloc::vec![ent("z", false, 7, 0), ent("y", false, 7, 0), ent("x", false, 7, 0)];
    let mut mm = Vec::new();
    sort_rows(&mut l, &mut mm, SortKey::Size, false);
    if names(&l) != "z,y,x" {
        return Err(alloc::format!("unstable: {}", names(&l)));
    }
    Ok("name|size|mtime|type")
}

/// Leg: the two new types — sniff, extension, the association, and the two renderers.
fn leg_types() -> Result<&'static str, String> {
    use crate::fs::assoc;
    use crate::fs::filetype as ft;
    let checks: [(&[u8], &str); 7] = [
        (b"# Title\n\nbody\n", ft::TEXT_MARKDOWN),
        (b"---\ntitle: x\n---\n# y\n", ft::TEXT_MARKDOWN),
        (b"{\"a\": 1}\n", ft::APP_JSON),
        (b"  [1, 2, {\"k\": true}]", ft::APP_JSON),
        (b"{ not json", ft::TEXT_PLAIN),
        (b"GIF89a\x01\x00\x01\x00\x00\x00\x00", ft::IMAGE_GIF),
        (b"hello\n", ft::TEXT_PLAIN),
    ];
    for (b, want) in checks.iter() {
        let got = ft::sniff(b).unwrap_or("none");
        if got != *want {
            return Err(alloc::format!("sniff {:?} -> {} want {}", core::str::from_utf8(&b[..b.len().min(12)]).unwrap_or("?"), got, want));
        }
    }
    for (n, want) in [("A.MD", ft::TEXT_MARKDOWN), ("b.markdown", ft::TEXT_MARKDOWN), ("c.JSON", ft::APP_JSON), ("d.gif", ft::IMAGE_GIF)] {
        if ft::by_extension(n) != Some(want) {
            return Err(alloc::format!("extension {} -> {:?}", n, ft::by_extension(n)));
        }
    }
    for (mime, op) in [(ft::TEXT_MARKDOWN, "markdown"), (ft::APP_JSON, "json"), (ft::IMAGE_GIF, "facet")] {
        match assoc::builtin(mime) {
            Some(r) if r.1 == op => {}
            other => return Err(alloc::format!("assoc {} -> {:?}", mime, other.map(|r| r.1))),
        }
        if op != "facet" && !super::openers::available(op) {
            return Err(alloc::format!("opener {} not available", op));
        }
    }
    let md = crate::video::fileview::richtext::markdown(b"# Head\n\n- one\n  - two\n1. three\nplain **bold** end\n");
    let t = core::str::from_utf8(&md.text).unwrap_or("");
    let bold = md.spans.iter().filter(|s| s.bold).count();
    if !t.starts_with("Head\n") || !t.contains("\n  - one\n    - two\n  1. three\n") || !t.contains("plain bold end") || bold < 2 {
        return Err(alloc::format!("markdown render text={:?} bold={}", t, bold));
    }
    let js = crate::video::fileview::richtext::json(b"{\"a\":[1,true,null],\"b\":{\"c\":\"x\"}}");
    let want = "{\n  \"a\": [\n    1,\n    true,\n    null\n  ],\n  \"b\": {\n    \"c\": \"x\"\n  }\n}\n";
    let jt = core::str::from_utf8(&js.text).unwrap_or("");
    if jt != want || js.spans.len() < 6 || js.note.is_some() {
        return Err(alloc::format!("json render {:?} spans={} note={:?}", jt, js.spans.len(), js.note));
    }
    let bad = crate::video::fileview::richtext::json(b"{\"a\": [1, }");
    if bad.note.is_none() {
        return Err(String::from("invalid json rendered without a note"));
    }
    Ok("+markdown,+json")
}

/// Leg (needs a home volume): a real Markdown file comes out `Markdown` in the listing's facts, and a
/// trashed file's row carries its origin. `Ok(None)` = no volume (SKIP).
fn leg_disk() -> Result<Option<(String, String)>, String> {
    let mt = crate::shell::vfs_mount_table();
    let home = crate::fs::trash::home_base();
    let dir = [home.as_str(), "/home"].iter().copied().find(|d| matches!(mt.stat(d), Ok(s) if matches!(s.kind, NodeKind::Dir)));
    let Some(dir) = dir else { return Ok(None) };
    let k = KERNEL_PRINCIPAL;
    let md = join(dir, "Q2NOTE.MD");
    let _ = mt.unlink(&md, k);
    mt.create(&md, NodeKind::File, k).map_err(|e| alloc::format!("create {}: {:?}", md, e))?;
    let body = b"# QUARRY2\n- a list\n";
    mt.write(&md, 0, body, k).map_err(|e| alloc::format!("write {}: {:?}", md, e))?;
    let list = alloc::vec![DirEnt { name: String::from("Q2NOTE.MD"), kind: NodeKind::File, size: body.len() as u64, mtime: None }];
    let meta = compute_meta(&mt, dir, &list, &[]);
    let ty = meta.first().map(|m| m.type_name.clone()).unwrap_or_default();
    if meta.first().map(|m| m.mime.as_str()) != Some(crate::fs::filetype::TEXT_MARKDOWN) {
        let _ = mt.unlink(&md, k);
        return Err(alloc::format!("listing typed {} as {:?}", md, meta.first().map(|m| m.mime.clone())));
    }
    // ORIGIN: trash the file, read the Trash's facts, restore it, delete it.
    let origin = match crate::fs::trash::trash(&md) {
        Ok(name) => {
            let td = crate::fs::trash::trash_dir();
            let origins: Vec<(String, String)> = crate::fs::trash::entries().into_iter().map(|e| (e.name, e.orig)).collect();
            let tl = alloc::vec![DirEnt { name: name.clone(), kind: NodeKind::File, size: body.len() as u64, mtime: None }];
            let tm = compute_meta(&mt, &td, &tl, &origins);
            let got = tm.first().and_then(|m| m.origin.clone()).unwrap_or_default();
            let _ = crate::fs::trash::restore(&name);
            if !got.eq_ignore_ascii_case(&md) {
                let _ = mt.unlink(&md, k);
                return Err(alloc::format!("origin of {} read {:?} want {} (store={})", name, got, md, crate::fs::trash::store().name()));
            }
            alloc::format!("ok({})", crate::fs::trash::store().name())
        }
        Err(e) => alloc::format!("skip({})", e),
    };
    let _ = mt.unlink(&md, k);
    Ok(Some((ty, origin)))
}

/// M5 — `tests quarry2`.
///
/// `:: QUARRY2: columns=type,origin sort=name|size|mtime|type types=+markdown,+json gif_frames=<n|skip> -> PASS ::`
pub fn selftest() {
    let mut fails: Vec<String> = Vec::new();
    let mut take = |r: Result<&'static str, String>, what: &str| -> String {
        match r {
            Ok(s) => String::from(s),
            Err(e) => {
                serial_println!("[quarry2] fixture {} FAIL: {}", what, e);
                fails.push(String::from(what));
                String::from("FAIL")
            }
        }
    };
    let columns = take(leg_columns(), "columns");
    let sort = take(leg_sort(), "sort");
    let types = take(leg_types(), "types");
    #[cfg(feature = "facet")]
    let (gif, stepper) = match crate::video::facet::anim::selftest_leg() {
        Ok((frames, stepper)) => (frames.map(|n| alloc::format!("{}", n)).unwrap_or_else(|| String::from("skip")), String::from(stepper)),
        Err(e) => {
            serial_println!("[quarry2] fixture frames FAIL: {}", e);
            fails.push(String::from("frames"));
            (String::from("FAIL"), String::from("FAIL"))
        }
    };
    #[cfg(not(feature = "facet"))]
    let (gif, stepper) = (String::from("skip"), String::from("skip(no-facet)"));
    let (disk_ty, origin) = match leg_disk() {
        Ok(Some((t, o))) => (t, o),
        Ok(None) => (String::from("skip"), String::from("skip(no-volume)")),
        Err(e) => {
            serial_println!("[quarry2] fixture disk FAIL: {}", e);
            fails.push(String::from("disk"));
            (String::from("FAIL"), String::from("FAIL"))
        }
    };
    serial_println!(
        ":: QUARRY2: columns={} sort={} types={} gif_frames={} stepper={} listing_type={} origin={} -> {} ::",
        columns,
        sort,
        types,
        gif,
        stepper,
        disk_ty.replace(' ', "_"),
        origin,
        if fails.is_empty() { "PASS" } else { "FAIL" }
    );
}
