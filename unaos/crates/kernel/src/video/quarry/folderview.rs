// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Matrix — kernel-by-ruling R50
//!
//! FOLDERVIEW (rmbp-ledger B424) — a folder REMEMBERS ITS VIEW, on the folder (MACPARITY §16 B6; Be's Tracker
//! kept `_trk/pinfo` and `_trk/columns` as attributes of the folder, so the view travelled with it).
//!
//! * **The mode** is QUARRY3's list/icons switcher (`toolbar::View`): applied on entering a folder, latched when
//!   the switcher changes it.
//! * **The keys** (attributes ON the folder, written as the kernel — the system's bookkeeping about the folder,
//!   through the one VFS attribute API ATTRCOLUMNS uses; no dotfile, no second store, R79):
//!   [`MODE`] `list|icons` · [`COLUMNS`] `size=9,modified=16,type=12,origin=28` · [`SORT`] `name|size|mtime|type`
//!   (`-` prefix = descending, `@<key>` = an ATTRCOLUMNS attribute column) · [`FRAME`] `x,y,w,h` (the outer frame,
//!   panel px). The attribute column SET stays ATTRCOLUMNS' `una:view`.
//! * **Resolve** on entering a folder: its own keys, else the nearest ancestor's, else the DEFAULT — Principia's
//!   `quarry` namespace (QUARRY2's widths/sort + `view.mode`), which only `Use as Default` writes. A volume
//!   without attributes (the boot FAT, R99) answers `Unsupported`: it inherits and is NEVER written.
//! * **Writes are latched** (a header press, `s`, `[`/`]`, a mode change, a window move that settled) and drained
//!   on Quarry's service pass — once per change, never per frame (R96). Leaving a folder flushes its latch.
//! * **The frame** is polled from the service pass (a read, every [`POLL_MS`]), saved once it has been still for
//!   [`SETTLE_MS`] or at close, and restored (clamped to the panel) when Quarry's window appears.
//! * **`View` menu** (winmenu, the bar's one framework): `Use as Default`, `Reset to Default`. A pick latches;
//!   the service pass does the I/O.
//!
//! Lock order: `MODEL` → `COLS` → `ATTRS` → [`FV`] (a leaf: nothing is called while it is held).
//! Witness: `:: FOLDERVIEW: folder=<p> mode=<m> cols=<n> sort=<k> frame=<w>x<h> restored=<1|0> ::` when Quarry's
//! window appears; `tests folderview` ([`selftest`]). Design: `docs/dev/evidence/rmbp-1005/folderview.md`.

use super::columns::{ColState, SortKey, Widths, COLS, DEFAULT_WIDTHS, NS};
use super::*;
use crate::fs::vfs::{AttrValue, MountTable, VfsError, KERNEL_PRINCIPAL};
use crate::video::winmenu;
use core::sync::atomic::AtomicU32;

pub const MODE: &str = "una:view.mode";
pub const COLUMNS: &str = "una:view.columns";
pub const SORT: &str = "una:view.sort";
pub const FRAME: &str = "una:view.frame";
const KEYS: [&str; 4] = [MODE, COLUMNS, SORT, FRAME];

/// The frame poll's period (a read of the window table, never a write).
const POLL_MS: u64 = 250;
/// A moved window is saved once it has been still this long (or at close).
const SETTLE_MS: u64 = 1000;
/// A latched view change is written this long after the last change (a held `]` is one write).
const LATCH_MS: u64 = 400;
/// Ancestors the resolve walks at most.
const WALK_MAX: usize = 16;

/// One folder's view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub mode: String,
    pub widths: Widths,
    pub sort: SortKey,
    pub desc: bool,
    /// An ATTRCOLUMNS column sort (its attribute key), over `sort`.
    pub attr_sort: Option<String>,
    /// The outer frame `(x, y, w, h)` in panel px.
    pub frame: Option<(usize, usize, usize, usize)>,
}

// ── The codec (pure) ────────────────────────────────────────────────────────────────────────────

fn mode_ok(s: &str) -> bool {
    matches!(s, "list" | "icons" | "columns") // COLUMNSVIEW (B436)
}

pub fn enc_columns(w: &Widths) -> String {
    alloc::format!("size={},modified={},type={},origin={}", w.size, w.modified, w.ty, w.origin)
}

pub fn dec_columns(s: &str, base: Widths) -> Widths {
    let mut w = base;
    for kv in s.split(',') {
        let mut it = kv.splitn(2, '=');
        let (Some(k), Some(v)) = (it.next(), it.next()) else { continue };
        let Ok(n) = v.trim().parse::<usize>() else { continue };
        let n = n.clamp(4, 64);
        match k.trim() {
            "size" => w.size = n,
            "modified" => w.modified = n,
            "type" => w.ty = n,
            "origin" => w.origin = n,
            _ => {}
        }
    }
    w
}

pub fn enc_sort(v: &View) -> String {
    match &v.attr_sort {
        Some(k) => alloc::format!("{}@{}", if v.desc { "-" } else { "" }, k),
        None => alloc::format!("{}{}", if v.desc { "-" } else { "" }, v.sort.name()),
    }
}

/// `(sort, desc, attr_sort)`; `None` for a value this build cannot read.
pub fn dec_sort(s: &str) -> Option<(SortKey, bool, Option<String>)> {
    let (desc, rest) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    if let Some(k) = rest.strip_prefix('@') {
        return (!k.is_empty()).then(|| (SortKey::Name, desc, Some(String::from(k))));
    }
    SortKey::ALL.iter().copied().find(|k| k.name() == rest).map(|k| (k, desc, None))
}

pub fn enc_frame(f: (usize, usize, usize, usize)) -> String {
    alloc::format!("{},{},{},{}", f.0, f.1, f.2, f.3)
}

pub fn dec_frame(s: &str) -> Option<(usize, usize, usize, usize)> {
    let mut n = s.split(',').map(|p| p.trim().parse::<usize>().ok());
    let f = (n.next()??, n.next()??, n.next()??, n.next()??);
    (n.next().is_none() && f.2 > 0 && f.3 > 0).then_some(f)
}

/// The frame's origin, clamped so the whole frame sits on a `pw x ph` panel (`wm::move_to` clamps again).
pub fn clamp_frame(f: (usize, usize, usize, usize), pw: usize, ph: usize) -> (usize, usize) {
    (f.0.min(pw.saturating_sub(f.2)), f.1.min(ph.saturating_sub(f.3)))
}

/// Overlay the keys of one folder's attribute list onto `v`; how many keys it carried.
fn overlay(v: &mut View, attrs: &[(String, AttrValue)]) -> usize {
    let mut n = 0;
    for (k, val) in attrs.iter() {
        let AttrValue::Str(s) = val else { continue };
        match k.as_str() {
            MODE if mode_ok(s) => {
                v.mode = s.clone();
                n += 1;
            }
            COLUMNS => {
                v.widths = dec_columns(s, v.widths);
                n += 1;
            }
            SORT => {
                if let Some((k, d, a)) = dec_sort(s) {
                    v.sort = k;
                    v.desc = d;
                    v.attr_sort = a;
                    n += 1;
                }
            }
            FRAME => {
                if let Some(f) = dec_frame(s) {
                    v.frame = Some(f);
                    n += 1;
                }
            }
            _ => {}
        }
    }
    n
}

// ── The default (Principia's) and the resolve ────────────────────────────────────────────────

/// The default view: QUARRY2's preference keys plus `view.mode` (`prefs::get` does no I/O).
pub fn default_view() -> View {
    let mut w = DEFAULT_WIDTHS;
    for (key, slot) in [("col.size", 0usize), ("col.modified", 1), ("col.type", 2), ("col.origin", 3)] {
        if let Some(n) = crate::prefs::get(NS, key).and_then(|v| v.as_int()) {
            let n = (n.max(0) as usize).clamp(4, 64);
            match slot {
                0 => w.size = n,
                1 => w.modified = n,
                2 => w.ty = n,
                _ => w.origin = n,
            }
        }
    }
    let sort = crate::prefs::get(NS, "sort.key")
        .and_then(|v| v.as_str().and_then(|s| SortKey::ALL.iter().copied().find(|k| k.name() == s)))
        .unwrap_or(SortKey::Name);
    let desc = crate::prefs::get(NS, "sort.desc").and_then(|v| v.as_bool()).unwrap_or(false);
    let mode = crate::prefs::get(NS, "view.mode")
        .and_then(|v| v.as_str().filter(|s| mode_ok(s)).map(String::from))
        .unwrap_or_else(|| String::from("list"));
    View { mode, widths: w, sort, desc, attr_sort: None, frame: None }
}

fn parent(p: &str) -> Option<&str> {
    if p == "/" || p.is_empty() {
        return None;
    }
    let t = p.trim_end_matches('/');
    match t.rfind('/') {
        Some(0) => Some("/"),
        Some(i) => Some(&t[..i]),
        None => None,
    }
}

/// Resolve `dir`'s view: `(view, source, writable)` — the source is `own`, `inherit:<dir>` or `default`, and
/// `writable` says the folder takes attributes (a FAT folder is `false` and is never written).
pub fn resolve(mt: &MountTable, dir: &str) -> (View, String, bool) {
    let mut v = default_view();
    let mut writable = false;
    let mut cur = Some(dir);
    for depth in 0..WALK_MAX {
        let Some(d) = cur else { break };
        match mt.list_attrs(d, KERNEL_PRINCIPAL) {
            Ok(attrs) => {
                if depth == 0 {
                    writable = true;
                }
                if overlay(&mut v, &attrs) > 0 {
                    let src = if depth == 0 { String::from("own") } else { alloc::format!("inherit:{}", d) };
                    // An attribute-column sort names a column of THAT folder; an heir sorts by its default key.
                    if depth > 0 {
                        v.attr_sort = None;
                    }
                    return (v, src, writable);
                }
            }
            Err(VfsError::Unsupported) | Err(_) => {}
        }
        cur = parent(d);
    }
    (v, String::from("default"), writable)
}

/// Write `v` onto `dir` (one transaction). The caller has checked the folder takes attributes.
fn save(mt: &MountTable, dir: &str, v: &View) -> Result<(), VfsError> {
    let kv = [
        (String::from(MODE), Some(AttrValue::Str(v.mode.clone()))),
        (String::from(COLUMNS), Some(AttrValue::Str(enc_columns(&v.widths)))),
        (String::from(SORT), Some(AttrValue::Str(enc_sort(v)))),
        (String::from(FRAME), v.frame.map(|f| AttrValue::Str(enc_frame(f)))),
    ];
    mt.set_attrs(dir, &kv, KERNEL_PRINCIPAL)
}

/// Remove the folder's view keys (and ATTRCOLUMNS' `una:view`): the folder inherits again.
fn clear(mt: &MountTable, dir: &str) -> Result<(), VfsError> {
    let mut kv: Vec<(String, Option<AttrValue>)> = KEYS.iter().map(|k| (String::from(*k), None)).collect();
    kv.push((String::from(crate::fs::attrfacts::VIEW), None));
    mt.set_attrs(dir, &kv, KERNEL_PRINCIPAL)
}

fn cols_n(trash: bool) -> usize {
    4 + trash as usize + super::attrcols::ATTRS.lock().keys.len()
}

// ── The state ───────────────────────────────────────────────────────────────────────────────────

struct Fv {
    dir: String,
    src: String,
    writable: bool,
    mode: String,
    /// The attribute sort the resolved view asked for, applied once ATTRCOLUMNS has read the folder's keys.
    want_attr: Option<String>,
    frame: Option<(usize, usize, usize, usize)>,
    dirty: bool,
    dirty_at: u64,
    pending: Vec<(String, View)>,
    win: u32,
    polled_at: u64,
    last_frame: Option<(usize, usize, usize, usize)>,
    frame_moved_at: Option<u64>,
}

static FV: spin::Mutex<Fv> = spin::Mutex::new(Fv {
    dir: String::new(),
    src: String::new(),
    writable: false,
    mode: String::new(),
    want_attr: None,
    frame: None,
    dirty: false,
    dirty_at: 0,
    pending: Vec::new(),
    win: wm::WIN_NONE,
    polled_at: 0,
    last_frame: None,
    frame_moved_at: None,
});

/// The menu pick, latched for the service pass (1 = Use as Default, 2 = Reset to Default).
static ACTION: AtomicU32 = AtomicU32::new(0);

/// The current folder's view mode (`list` / `icons`).
pub fn mode() -> String {
    let m = FV.lock().mode.clone();
    if m.is_empty() { String::from("list") } else { m }
}

/// QUARRY3's view switcher: the folder's mode changed (latched; written on the service pass).
pub fn set_mode(m: &str) {
    if !mode_ok(m) {
        return;
    }
    FV.lock().mode = String::from(m);
    changed();
}

/// A view change in the current folder (header press, `s`, `[`/`]`): latched, written on the service pass.
pub fn changed() {
    let mut f = FV.lock();
    f.dirty = true;
    f.dirty_at = crate::arch::ms();
}

/// The current view, read from the column state (the caller holds no lock below `COLS`).
fn snapshot(st: &ColState, f: &Fv, attr: Option<String>) -> View {
    View {
        mode: if f.mode.is_empty() { String::from("list") } else { f.mode.clone() },
        widths: st.widths,
        sort: st.key,
        desc: st.desc,
        attr_sort: attr,
        frame: f.frame,
    }
}

fn attr_sort_key() -> Option<String> {
    super::attrcols::sort_index().and_then(super::attrcols::key_at)
}

/// `columns::after_show`, before the listing's facts: a NEW folder flushes the old folder's latch, resolves its
/// own view and applies it to the column state. Inside `MODEL` → `COLS`.
pub(super) fn enter(cwd: &str, st: &mut ColState) {
    let attr_now = attr_sort_key();
    let searching = super::toolbar::search_active();
    let flush = {
        let mut f = FV.lock();
        if f.dir == cwd && !searching {
            return;
        }
        let out = (f.dirty && f.writable && !f.dir.is_empty()).then(|| (f.dir.clone(), snapshot(st, &f, attr_now)));
        f.dirty = false;
        if let Some(o) = out.clone() {
            f.pending.push(o);
        }
        if searching {
            // QUARRY3's search hits are listed as `/`: not a folder, so nothing is resolved or saved for them.
            f.dir.clear();
            return;
        }
        out.is_some()
    };
    let mt = crate::shell::vfs_mount_table();
    let (v, src, writable) = resolve(&mt, cwd);
    st.widths = v.widths;
    st.key = v.sort;
    st.desc = v.desc;
    serial_println!(
        "[folderview] enter dir={} src={} writable={} mode={} sort={} cols={}{}",
        cwd, src, writable as u8, v.mode, enc_sort(&v), enc_columns(&v.widths),
        if flush { " (flushed the last folder's change)" } else { "" }
    );
    super::toolbar::apply_mode(&v.mode);
    let mut f = FV.lock();
    f.dir = String::from(cwd);
    f.src = src;
    f.writable = writable;
    f.mode = v.mode;
    f.want_attr = v.attr_sort;
    f.frame = v.frame.or(f.last_frame);
}

/// `columns::after_show`, after ATTRCOLUMNS read the folder's keys: apply a resolved attribute-column sort.
pub(super) fn after_attrs() {
    let Some(k) = FV.lock().want_attr.take() else { return };
    let i = super::attrcols::ATTRS.lock().keys.iter().position(|x| *x == k);
    if i.is_some() {
        super::attrcols::set_sort(i);
    }
}

// ── The service pass ────────────────────────────────────────────────────────────────────────────

static VIEW_ITEMS: [winmenu::MenuItem; 2] = [
    winmenu::MenuItem { id: 1, label: "Use as Default", flags: 0 },
    winmenu::MenuItem { id: 2, label: "Reset to Default", flags: 0 },
];
static VIEW_TREE: [winmenu::MenuTitle; 1] = [winmenu::MenuTitle { label: "View", items: &VIEW_ITEMS }];

fn on_pick(id: u32) {
    ACTION.store(id, Ordering::Release);
}

/// Quarry's service pass (chained from `columns::service`): the menu's latch, the window's frame, the
/// latched view writes. Never from a router.
pub fn service() {
    let now = crate::arch::ms();
    match ACTION.swap(0, Ordering::AcqRel) {
        1 => use_as_default(),
        2 => reset(),
        _ => {}
    }
    window(now);
    let due = {
        let f = FV.lock();
        f.dirty && now.saturating_sub(f.dirty_at) >= LATCH_MS
    };
    if due {
        let attr = attr_sort_key();
        let st = COLS.lock();
        let mut f = FV.lock();
        f.dirty = false;
        if f.writable && !f.dir.is_empty() {
            let o = (f.dir.clone(), snapshot(&st, &f, attr));
            f.pending.push(o);
        }
    }
    drain();
}

fn drain() {
    let batch: Vec<(String, View)> = core::mem::take(&mut FV.lock().pending);
    if batch.is_empty() {
        return;
    }
    let mt = crate::shell::vfs_mount_table();
    for (dir, v) in batch.iter() {
        let r = save(&mt, dir, v);
        serial_println!(
            "[folderview] save dir={} mode={} cols={} sort={} frame={} ok={}",
            dir, v.mode, enc_columns(&v.widths), enc_sort(v),
            v.frame.map(enc_frame).unwrap_or_else(|| String::from("-")),
            match &r { Ok(()) => String::from("1"), Err(e) => alloc::format!("0({:?})", e) }
        );
    }
}

/// The window's life as the service pass sees it: a new window publishes the `View` menu and gets its folder's
/// frame back; an open window's frame is polled; a closed one flushes a moved frame.
fn window(now: u64) {
    let win = super::WIN.load(Ordering::Relaxed);
    let was = FV.lock().win;
    if win != was && was != wm::WIN_NONE {
        winmenu::clear(was);
        let mut f = FV.lock();
        f.win = wm::WIN_NONE;
        if f.frame_moved_at.take().is_some() {
            f.dirty = true;
            f.dirty_at = 0;
        }
    }
    if win == wm::WIN_NONE {
        return;
    }
    if win != was {
        appeared(win);
        return;
    }
    {
        let mut f = FV.lock();
        if now.saturating_sub(f.polled_at) < POLL_MS {
            if let Some(t) = f.frame_moved_at {
                if now.saturating_sub(t) >= SETTLE_MS {
                    f.frame_moved_at = None;
                    f.dirty = true;
                    f.dirty_at = 0;
                }
            }
            return;
        }
        f.polled_at = now;
    }
    let fr = wm::frame_of(win);
    let mut f = FV.lock();
    if fr.is_some() && fr != f.last_frame {
        f.last_frame = fr;
        f.frame = fr;
        f.frame_moved_at = Some(now);
    }
}

fn appeared(win: u32) {
    let published = winmenu::publish(win, &VIEW_TREE, on_pick);
    let want = FV.lock().frame;
    let panel = crate::video::panel_info_nonblocking().map(|i| (i.width, i.height));
    let mut restored = false;
    if let (Some(fr), Some((pw, ph))) = (want, panel) {
        let (x, y) = clamp_frame(fr, pw, ph);
        restored = wm::move_to(win, x + wm::BORDER(), y + wm::TITLE_H() + wm::BORDER());
    }
    let now_fr = wm::frame_of(win);
    let (dir, mode, sort) = {
        let mut f = FV.lock();
        f.win = win;
        f.last_frame = now_fr;
        f.frame_moved_at = None;
        if f.frame.is_none() {
            f.frame = now_fr;
        }
        (f.dir.clone(), f.mode.clone(), f.want_attr.clone())
    };
    let (key, desc, trash) = {
        let st = COLS.lock();
        (st.key, st.desc, st.trash)
    };
    let sort = match attr_sort_key().or(sort) {
        Some(k) => alloc::format!("{}@{}", if desc { "-" } else { "" }, k),
        None => alloc::format!("{}{}", if desc { "-" } else { "" }, key.name()),
    };
    let (w, h) = now_fr.map(|f| (f.2, f.3)).unwrap_or((0, 0));
    serial_println!(
        ":: FOLDERVIEW: folder={} mode={} cols={} sort={} frame={}x{} restored={} :: menu={}",
        dir, if mode.is_empty() { "list" } else { mode.as_str() }, cols_n(trash), sort, w, h, restored as u8, published as u8
    );
}

/// `View ▸ Use as Default`: the current folder's view becomes Principia's default (QUARRY2's keys + `view.mode`).
fn use_as_default() {
    let (dir, mode) = {
        let f = FV.lock();
        (f.dir.clone(), if f.mode.is_empty() { String::from("list") } else { f.mode.clone() })
    };
    let ok = crate::prefs::set(NS, "view.mode", crate::prefs::PrefValue::Str(mode.clone())).is_ok();
    super::columns::persist_default();
    serial_println!("[folderview] menu use-as-default dir={} mode={} mode_saved={} (widths/sort on the service pass)", dir, mode, ok as u8);
}

/// `View ▸ Reset to Default`: the folder's view keys go; the folder inherits again (a FAT folder had none).
fn reset() {
    let (dir, writable) = {
        let mut f = FV.lock();
        f.dirty = false;
        let d = f.dir.clone();
        f.pending.retain(|(p, _)| *p != d);
        (d, f.writable)
    };
    let r = if writable { Some(clear(&crate::shell::vfs_mount_table(), &dir)) } else { None };
    serial_println!(
        "[folderview] menu reset dir={} cleared={}",
        dir,
        match &r { Some(Ok(())) => String::from("1"), Some(Err(e)) => alloc::format!("0({:?})", e), None => String::from("0(no-attributes)") }
    );
    {
        let mut f = FV.lock();
        f.dir.clear();
        f.frame = None;
    }
    super::attrcols::ATTRS.lock().dir.clear();
    if let Some(m) = MODEL.lock().as_mut() {
        let c = m.cwd.clone();
        m.invalidate();
        m.show(&c);
        m.settle();
    }
    repaint();
}

// ── `tests folderview` ──────────────────────────────────────────────────────────────────────────

/// Registration, once (rides `columns::ensure_tests`; no tests.rs line).
pub fn ensure_tests() {
    use core::sync::atomic::AtomicBool;
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("folderview", selftest);
    }
}

fn leg_codec() -> Result<(), &'static str> {
    let w = Widths { size: 10, modified: 17, ty: 13, origin: 30 };
    if dec_columns(&enc_columns(&w), DEFAULT_WIDTHS) != w {
        return Err("columns");
    }
    let v = View { mode: String::from("list"), widths: w, sort: SortKey::Mtime, desc: true, attr_sort: None, frame: None };
    if dec_sort(&enc_sort(&v)) != Some((SortKey::Mtime, true, None)) {
        return Err("sort");
    }
    if dec_sort("@media:duration_ms") != Some((SortKey::Name, false, Some(String::from("media:duration_ms")))) {
        return Err("attr-sort");
    }
    if dec_frame(&enc_frame((10, 20, 800, 600))) != Some((10, 20, 800, 600)) || dec_frame("1,2,0,4").is_some() {
        return Err("frame");
    }
    if clamp_frame((5000, 5000, 800, 600), 1920, 1200) != (1120, 600) || clamp_frame((10, 20, 800, 600), 1920, 1200) != (10, 20) {
        return Err("clamp");
    }
    if parent("/a/b") != Some("/a") || parent("/a") != Some("/") || parent("/").is_some() {
        return Err("parent");
    }
    Ok(())
}

/// `tests folderview`: the codec (pure); then on the UnaFS test folder — a view saved on the folder resolves as
/// `own`, a child path inherits it, `Reset` clears it back to the parent's or the default, and a FAT folder
/// resolves `writable=0` (no write is attempted on it). Every key is restored. R80: run only when asked.
pub fn selftest() {
    let codec = leg_codec();
    let mt = crate::shell::vfs_mount_table();
    let dir = crate::fs::volumes::TESTF_DIRS.iter().copied().find(|d| mt.stat(d).is_ok());
    let fat = ["/volumes/boot", "/boot"].iter().copied().find(|d| mt.stat(d).is_ok());
    let fat_writable = fat.map(|d| resolve(&mt, d).2 as u32).unwrap_or(0);
    let Some(dir) = dir.filter(|d| !matches!(mt.list_attrs(d, KERNEL_PRINCIPAL), Err(VfsError::Unsupported))) else {
        serial_println!(
            ":: FOLDERVIEW: codec={} fat_writable={} -> SKIP :: reason=no-unafs-test-folder ::",
            if codec.is_ok() { "ok" } else { "FAIL" }, fat_writable
        );
        return;
    };
    let old: Vec<(String, Option<AttrValue>)> =
        KEYS.iter().map(|k| (String::from(*k), mt.get_attr(dir, k, KERNEL_PRINCIPAL).ok())).collect();
    let mut want = default_view();
    want.mode = String::from("icons");
    want.sort = SortKey::Size;
    want.desc = true;
    want.widths.size = 11;
    want.frame = Some((40, 60, 900, 640));
    let saved = save(&mt, dir, &want).is_ok();
    let (own, own_src, w_ok) = resolve(&mt, dir);
    let own_ok = saved && w_ok && own_src == "own" && own == want;
    let child = alloc::format!("{}/TEST.MD", dir);
    let (heir, heir_src, _) = resolve(&mt, &child);
    let inherit_ok = heir_src == alloc::format!("inherit:{}", dir) && heir.sort == SortKey::Size && heir.mode == "icons";
    let cleared = clear(&mt, dir).is_ok();
    let (after, after_src, _) = resolve(&mt, dir);
    let reset_ok = cleared && after_src != "own" && after.frame != want.frame;
    let _ = mt.set_attrs(dir, &old, KERNEL_PRINCIPAL);
    let (fr, restored) = match crate::video::panel_info_nonblocking() {
        Some(i) => {
            let (x, y) = clamp_frame(own.frame.unwrap_or((0, 0, 0, 0)), i.width, i.height);
            (own.frame.unwrap_or((0, 0, 0, 0)), (x, y) == (40, 60))
        }
        None => (own.frame.unwrap_or((0, 0, 0, 0)), false),
    };
    let pass = codec.is_ok() && own_ok && inherit_ok && reset_ok && fat_writable == 0;
    serial_println!(
        ":: FOLDERVIEW: folder={} mode={} cols={} sort={} frame={}x{} restored={} codec={} inherit={} reset={} fat_writable={} -> {} :: src={} heir={} after={}",
        dir, own.mode, cols_n(false), enc_sort(&own), fr.2, fr.3, restored as u8,
        match codec { Ok(()) => "ok", Err(e) => e },
        if inherit_ok { "ok" } else { "FAIL" },
        if reset_ok { "ok" } else { "FAIL" },
        fat_writable,
        if pass { "PASS" } else { "FAIL" },
        own_src, heir_src, after_src
    );
}
