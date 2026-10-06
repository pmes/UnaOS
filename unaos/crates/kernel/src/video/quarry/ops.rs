// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! QUARRYOPS (R75) — Quarry's FILE OPERATIONS: right-click menu, inline rename field, copy/paste,
//! delete, new folder, info. A child module of [`super`] (`live`) so it reads the model's private
//! fields; compiled only with `quarry` (and reachable from a window only with `wc`/`desktop_firmware`).
//!
//! Every op is the SAME seams the shell verbs use (`MountTable::{create,unlink,remove_dir,rename,read,
//! write}`), under the DIRNS namespace rule: a user acts under `/home/<user>`; anything else is refused
//! with a `notice_show` saying why (B229). Each op prints `[quarry] op=<name> src= dst= ok= reason=`
//! and refreshes the listing. Pure op bodies take paths only (no UI), so the `quarryops` fixture drives
//! exactly what the menu drives.
//!
//! Lock discipline: UI state (MENU/EDIT/CLIP) are leaf locks; the op bodies run with NO Quarry lock
//! held; the refresh re-takes `MODEL` afterwards.

use super::*;
use crate::fs::vfs::{MountTable, VfsError, KERNEL_PRINCIPAL};
use core::sync::atomic::AtomicBool;

const P: &str = KERNEL_PRINCIPAL;
const MAX_TREE_DEPTH: usize = 6;
const COPY_CHUNK: usize = 32 * 1024;

fn mt() -> MountTable {
    crate::shell::vfs_mount_table()
}

fn err_s(e: VfsError) -> String {
    alloc::format!("{:?}", e)
}

// ── namespace (DIRNS) ───────────────────────────────────────────────────────────────────────────

/// The session user's name, when there is one.
fn session_user() -> Option<String> {
    #[cfg(all(target_arch = "x86_64", feature = "login"))]
    {
        let mut b = [0u8; 32];
        let n = crate::arch::x86_64::syscall::session_name(&mut b)?;
        return core::str::from_utf8(&b[..n]).ok().map(String::from);
    }
    #[allow(unreachable_code)]
    None
}

/// The folder every op is confined to: `/home/<user>`, or `/home` with no session.
fn home_base() -> String {
    match session_user() {
        Some(u) => alloc::format!("/home/{}", u),
        None => String::from("/home"),
    }
}

fn starts_ci(path: &str, prefix: &str) -> bool {
    path.len() >= prefix.len()
        && path.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
        && (path.len() == prefix.len() || path.as_bytes()[prefix.len()] == b'/')
}

/// DIRNS: is `path` inside the user's namespace? `Err(why)` is the sentence the NOTICE shows.
/// `allow_base` false refuses the namespace root itself (never delete/rename the home).
fn ns_check(path: &str, allow_base: bool) -> Result<(), String> {
    if path.split('/').any(|c| c == "..") {
        return Err(String::from("a path with .. leaves the home folder"));
    }
    let base = home_base();
    if !starts_ci(path, &base) {
        return Err(alloc::format!("files are changed only under {}", base));
    }
    if !allow_base && path.len() == base.len() {
        return Err(String::from("the home folder itself cannot be changed"));
    }
    Ok(())
}

fn valid_name(n: &str) -> Result<(), String> {
    if n.is_empty() || n == "." || n == ".." || n.contains('/') || n.len() > 255 {
        return Err(String::from("bad-name"));
    }
    Ok(())
}

// ── the op bodies ───────────────────────────────────────────────────────────────────────────────

fn exists(t: &MountTable, p: &str) -> bool {
    t.stat(p).is_ok()
}

pub fn op_mkdir(path: &str) -> Result<(), String> {
    ns_check(path, false)?;
    valid_name(&leaf(path))?;
    let t = mt();
    if exists(&t, path) {
        return Err(String::from("exists"));
    }
    t.create(path, NodeKind::Dir, P).map(|_| ()).map_err(err_s)
}

pub fn op_rename(src: &str, dst: &str) -> Result<(), String> {
    ns_check(src, false)?;
    ns_check(dst, false)?;
    valid_name(&leaf(dst))?;
    let t = mt();
    if exists(&t, dst) {
        return Err(String::from("exists"));
    }
    let before = crate::fs::fat::sector_write_count(); // LFNMV2 — the same counter line the shell's `mv` prints
    let r = t.rename(src, dst, P);
    if crate::fs::fat::is_long_name(&leaf(dst)) {
        serial_println!("[fs] mv {} -> {} lfn=1 ok={} sectors_written={}", src, dst, r.is_ok(), crate::fs::fat::sector_write_count().wrapping_sub(before));
    }
    r.map_err(err_s)
}

fn delete_tree(t: &MountTable, path: &str, depth: usize) -> Result<(), String> {
    let st = t.stat(path).map_err(err_s)?;
    if matches!(st.kind, NodeKind::File) {
        return t.unlink(path, P).map_err(err_s);
    }
    if depth > MAX_TREE_DEPTH {
        return Err(String::from("too-deep"));
    }
    for e in t.read_dir(path).map_err(err_s)? {
        if e.name == "." || e.name == ".." {
            continue;
        }
        delete_tree(t, &join(path, &e.name), depth + 1)?;
    }
    t.remove_dir(path, P).map_err(err_s)
}

pub fn op_delete(path: &str) -> Result<(), String> {
    ns_check(path, false)?;
    delete_tree(&mt(), path, 0)
}

fn copy_tree(t: &MountTable, src: &str, dst: &str, depth: usize) -> Result<(), String> {
    let st = t.stat(src).map_err(err_s)?;
    if matches!(st.kind, NodeKind::Dir) {
        if depth > MAX_TREE_DEPTH {
            return Err(String::from("too-deep"));
        }
        t.create(dst, NodeKind::Dir, P).map_err(err_s)?;
        for e in t.read_dir(src).map_err(err_s)? {
            if e.name == "." || e.name == ".." {
                continue;
            }
            copy_tree(t, &join(src, &e.name), &join(dst, &e.name), depth + 1)?;
        }
        return Ok(());
    }
    t.create(dst, NodeKind::File, P).map_err(err_s)?;
    let mut off = 0u64;
    loop {
        let chunk = t.read(src, off, COPY_CHUNK).map_err(err_s)?;
        if chunk.is_empty() {
            break;
        }
        let n = t.write(dst, off, &chunk, P).map_err(err_s)?;
        if n != chunk.len() {
            return Err(String::from("short-write"));
        }
        off += chunk.len() as u64;
    }
    Ok(())
}

/// `<stem>-copy<.ext>`, then `-copy2`.. until the name is free in `dir`.
fn unique_name(t: &MountTable, dir: &str, name: &str) -> String {
    if !exists(t, &join(dir, name)) {
        return String::from(name);
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 1..100 {
        let c = if n == 1 { alloc::format!("{}-copy{}", stem, ext) } else { alloc::format!("{}-copy{}{}", stem, n, ext) };
        if !exists(t, &join(dir, &c)) {
            return c;
        }
    }
    alloc::format!("{}-copy99{}", stem, ext)
}

/// Copy `src` into folder `dir` (a collision gets a `-copy` name). Returns the new path.
pub fn op_copy(src: &str, dir: &str) -> Result<String, String> {
    ns_check(dir, true)?;
    ns_check(src, true)?;
    let t = mt();
    if starts_ci(dir, src) {
        return Err(String::from("into-itself"));
    }
    let dst = join(dir, &unique_name(&t, dir, &leaf(src)));
    match copy_tree(&t, src, &dst, 0) {
        Ok(()) => Ok(dst),
        Err(e) => {
            let _ = delete_tree(&t, &dst, 0); // never leave a half copy
            Err(e)
        }
    }
}

fn log_op(op: &str, src: &str, dst: &str, r: &Result<(), String>) {
    match r {
        Ok(()) => serial_println!("[quarry] op={} src={} dst={} ok=1 reason=-", op, src, dst),
        Err(e) => serial_println!("[quarry] op={} src={} dst={} ok=0 reason={}", op, src, dst, e),
    }
}

/// B229 — a refusal the operator can read: the NOTICE (when the login stack is built) and the path bar.
fn refuse_notice(why: &str) {
    #[cfg(feature = "login")]
    crate::video::crystal::login::notice_show(b"Quarry", why.as_bytes());
    let _ = why;
}

// ── UI state ────────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Item {
    Open,
    Rename,
    Delete,
    DeletePerm,
    NewFolder,
    Copy,
    Paste,
    Info,
    ShowTrash,
    Restore,
    EmptyTrash,
}

impl Item {
    fn label(self) -> String {
        if self == Item::EmptyTrash {
            return if empty_armed() { String::from("Click again: Empty") } else { alloc::format!("Empty Trash ({} items)", crate::fs::trash::count()) };
        }
        String::from(match self {
            Item::Open => "Open",
            Item::Rename => "Rename",
            Item::Delete => "Move to Trash",
            Item::DeletePerm => "Delete Permanently",
            Item::NewFolder => "New Folder",
            Item::Copy => "Copy",
            Item::Paste => "Paste",
            Item::Info => "Get Info",
            Item::ShowTrash => "Show Trash",
            Item::Restore => "Restore",
            Item::EmptyTrash => "",
        })
    }
}

const ITEMS: [Item; 11] = [Item::Open, Item::Rename, Item::Delete, Item::DeletePerm, Item::NewFolder, Item::Copy, Item::Paste, Item::Info, Item::ShowTrash, Item::Restore, Item::EmptyTrash];

/// TRASH (R75) — `Empty Trash` is a TWO-STEP (no yes/no alert exists; `login::open_alert` is OK-only): the
/// first press arms it (uptime seconds + 1), a second within [`EMPTY_WINDOW_S`] empties.
static EMPTY_ARMED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
const EMPTY_WINDOW_S: u64 = 5;
/// TRASH (R75) — Shift+Delete = permanent. Quarry's key stream is bare bytes (no modifier), so whatever
/// owns the modifier state sets this (`ops::set_shift`); until wired, `D` (Shift+d) and the menu row do it.
static SHIFT_HELD: AtomicBool = AtomicBool::new(false);
pub fn set_shift(down: bool) {
    SHIFT_HELD.store(down, Ordering::Relaxed);
}
fn now_s() -> u64 {
    crate::clock::uptime_secs().unwrap_or(0)
}
fn empty_armed() -> bool {
    let a = EMPTY_ARMED.load(Ordering::Relaxed);
    a != 0 && now_s().saturating_sub(a - 1) <= EMPTY_WINDOW_S
}

/// The open context menu: source-pixel origin.
struct Menu {
    x: usize,
    y: usize,
}
static MENU: spin::Mutex<Option<Menu>> = spin::Mutex::new(None);
static MENU_UP: AtomicBool = AtomicBool::new(false);

#[derive(Clone)]
enum Target {
    Rename(String),
    NewFolder,
    /// ATTRCOLUMNS (B402): an attribute cell — `(path, key, int)`.
    Attr(String, String, bool),
}
struct Edit {
    buf: String,
    target: Target,
}
static EDIT: spin::Mutex<Option<Edit>> = spin::Mutex::new(None);
static CLIP: spin::Mutex<Option<String>> = spin::Mutex::new(None);

fn menu_w(g: &Geom) -> usize {
    22 * g.cell_w() + 2 * PAD()
}
fn menu_h(g: &Geom) -> usize {
    ITEMS.len() * g.row_h() + 2
}

/// Paint the menu and the edit field over the finished frame. Called at the end of `repaint_locked`.
pub fn paint_overlay(m: &Model, px: &mut [u32]) {
    let g = &m.geom;
    if let Some(e) = EDIT.lock().as_ref() {
        let li = g.list_pane().inner();
        let rh = g.row_h();
        let body_y = li.y + rh;
        let (y, label): (usize, &str) = match &e.target {
            Target::Rename(_) if m.list_sel >= m.list_scroll && m.list_sel < m.list_scroll + m.list_visible() => {
                (body_y + (m.list_sel - m.list_scroll) * rh, "")
            }
            Target::Rename(_) => (0, "Rename: "),
            Target::NewFolder => (0, "New folder: "),
            Target::Attr(..) if m.list_sel >= m.list_scroll && m.list_sel < m.list_scroll + m.list_visible() => {
                (body_y + (m.list_sel - m.list_scroll) * rh, "")
            }
            Target::Attr(..) => (0, ""),
        };
        let attr_label = match &e.target {
            Target::Attr(_, k, _) => alloc::format!("{}: ", k),
            _ => String::new(),
        };
        let label: &str = if attr_label.is_empty() { label } else { attr_label.as_str() };
        let (x, w) = if y == 0 { (0, g.w) } else { (li.x, li.w) };
        fill(px, g, x, y, w, rh, 0x00FF_FFFF);
        keyline(px, g, Rect { x, y, w, h: rh }, theme::ACCENT);
        let mut s: Vec<u8> = Vec::new();
        s.extend_from_slice(label.as_bytes());
        s.extend_from_slice(e.buf.as_bytes());
        s.push(b'_');
        text(px, g, x + PAD(), y + g.ts, &s, x + w, theme::CONTENT_TEXT);
    }
    if let Some(mn) = MENU.lock().as_ref() {
        let (w, h) = (menu_w(g), menu_h(g));
        fill(px, g, mn.x, mn.y, w, h, theme::BUTTON_FACE);
        keyline(px, g, Rect { x: mn.x, y: mn.y, w, h }, theme::FRAME_LINE);
        for (i, it) in ITEMS.iter().enumerate() {
            text(px, g, mn.x + PAD(), mn.y + 1 + i * g.row_h() + g.ts, it.label().as_bytes(), mn.x + w, theme::BUTTON_TEXT);
        }
    }
    super::getinfo::paint(m, px); // ATTRCOLUMNS (B402): the inspector
    super::attrcols::paint_menu(m, px); // ATTRCOLUMNS (B402): the header's Add column… menu
}

/// Panel point -> (source x, source y) inside Quarry's content, or None.
fn to_source(x: i32, y: i32) -> Option<(usize, usize)> {
    let id = WIN.load(Ordering::Relaxed);
    if id == wm::WIN_NONE {
        return None;
    }
    match wm::hit_test(x, y) {
        Some((w, _, _)) if w == id => {}
        _ => return None,
    }
    let info = wm::info(id)?;
    let scale = info.scale.max(1);
    if x < info.x as i32 || y < info.y as i32 {
        return None;
    }
    let (sx, sy) = ((x as usize - info.x) / scale, (y as usize - info.y) / scale);
    if sx >= info.w || sy >= info.h {
        return None;
    }
    Some((sx, sy))
}

/// The selected list entry as (absolute path, is_dir), and the cwd.
fn selection() -> Option<(String, String, bool)> {
    let guard = MODEL.lock();
    let m = guard.as_ref()?;
    let e = m.list.get(m.list_sel)?;
    Some((m.cwd.clone(), join(&m.cwd, &e.name), matches!(e.kind, NodeKind::Dir)))
}

fn cwd() -> Option<String> {
    MODEL.lock().as_ref().map(|m| m.cwd.clone())
}

/// Re-read the listing and (optionally) select `name`.
fn refresh(select: Option<&str>) {
    if let Some(m) = MODEL.lock().as_mut() {
        let c = m.cwd.clone();
        m.invalidate();
        m.show(&c);
        if let Some(n) = select {
            if let Some(i) = m.list.iter().position(|e| e.name.eq_ignore_ascii_case(n)) {
                m.list_sel = i;
            }
        }
        m.status = None;
        m.settle();
    }
}

fn say(s: String) {
    if let Some(m) = MODEL.lock().as_mut() {
        m.status = Some(s);
    }
}

// ── the right-click ─────────────────────────────────────────────────────────────────────────────

/// A secondary press. Selects the row under the pointer and opens the menu there. Consumed when the
/// press landed in Quarry's content.
pub fn right_press(x: i32, y: i32) -> bool {
    let Some((sx, sy)) = to_source(x, y) else { return false };
    wm::focus_changed(OWNER);
    {
        let mut guard = MODEL.lock();
        let Some(m) = guard.as_mut() else { return true };
        let g = &m.geom;
        let li = g.list_pane().inner();
        let body_y = li.y + g.row_h();
        if super::attrcols::on_header(m, sx, sy) {
            // ATTRCOLUMNS (B402): the header's right-click is the column menu, not the file menu.
            super::attrcols::open_menu(m, sx, sy);
            drop(guard);
            repaint();
            return true;
        }
        if sx >= li.x && sx < li.x + li.w && sy >= body_y {
            let i = m.list_scroll + (sy - body_y) / g.row_h();
            if i < m.list.len() {
                m.list_sel = i;
                m.focus = Pane::List;
            }
        }
        let (w, h) = (menu_w(g), menu_h(g));
        let mx = sx.min(g.w.saturating_sub(w));
        let my = sy.min(g.h.saturating_sub(h));
        *MENU.lock() = Some(Menu { x: mx, y: my });
        MENU_UP.store(true, Ordering::Release);
    }
    serial_println!("[quarry] menu open at=({},{})", sx, sy);
    repaint();
    true
}

/// A primary press while the menu is up: pick an item (consumed) or dismiss (falls through).
pub fn menu_press(x: i32, y: i32) -> bool {
    if super::attrcols::menu_up() {
        let hit = to_source(x, y);
        super::attrcols::menu_press_at(hit);
        return hit.is_some();
    }
    if super::getinfo::is_up() {
        // ATTRCOLUMNS (B402): any press dismisses the inspector; one inside Quarry is consumed by it.
        super::getinfo::dismiss();
        repaint();
        return to_source(x, y).is_some();
    }
    if !MENU_UP.load(Ordering::Acquire) {
        return false;
    }
    let mn = MENU.lock().take();
    MENU_UP.store(false, Ordering::Release);
    let Some(mn) = mn else { return false };
    let hit = to_source(x, y);
    let mut picked = None;
    if let Some((sx, sy)) = hit {
        let g = match MODEL.lock().as_ref() {
            Some(m) => (menu_w(&m.geom), m.geom.row_h()),
            None => return true,
        };
        if sx >= mn.x && sx < mn.x + g.0 && sy >= mn.y + 1 {
            let i = (sy - mn.y - 1) / g.1;
            if i < ITEMS.len() {
                picked = Some(ITEMS[i]);
            }
        }
    }
    match picked {
        Some(it) => run_item(it),
        None => repaint(),
    }
    hit.is_some()
}

fn run_item(it: Item) {
    let sel = selection();
    match it {
        Item::Open => {
            let act = {
                let mut guard = MODEL.lock();
                match guard.as_mut() {
                    Some(m) => {
                        let i = m.list_sel;
                        let a = m.activate_row(i);
                        m.settle();
                        a
                    }
                    None => Act::None,
                }
            };
            serial_println!("[quarry] op=open src={} dst=- ok=1 reason=-", sel.as_ref().map(|s| s.1.as_str()).unwrap_or("-"));
            run_act(act);
        }
        Item::Rename => start_rename(),
        Item::NewFolder => start_new_folder(),
        Item::Delete => do_trash(),
        Item::DeletePerm => do_delete(),
        Item::ShowTrash => do_show_trash(),
        Item::Restore => do_restore(),
        Item::EmptyTrash => do_empty_trash(),
        Item::Copy => do_copy(),
        Item::Paste => do_paste(),
        Item::Info => do_info(),
    }
    repaint();
}

fn start_rename() {
    let Some((_, path, _)) = selection() else { return };
    if let Err(why) = ns_check(&path, false) {
        log_op("rename", &path, "-", &Err(why.clone()));
        refuse_notice(&why);
        return;
    }
    *EDIT.lock() = Some(Edit { buf: leaf(&path), target: Target::Rename(path) });
}

fn start_new_folder() {
    let Some(c) = cwd() else { return };
    if let Err(why) = ns_check(&c, true) {
        log_op("mkdir", &c, "-", &Err(why.clone()));
        refuse_notice(&why);
        return;
    }
    *EDIT.lock() = Some(Edit { buf: String::new(), target: Target::NewFolder });
}

fn do_delete() {
    let Some((_, path, _)) = selection() else { return };
    let r = op_delete(&path);
    log_op("delete", &path, "-", &r);
    if let Err(e) = &r {
        if e.starts_with("files are") || e.starts_with("the home") || e.starts_with("a path") {
            refuse_notice(e);
        }
        say(alloc::format!("delete refused ({})", e));
    }
    refresh(None);
}

/// TRASH (R75): Delete = move to `.Trash` (index line appended).
fn do_trash() {
    let Some((_, path, _)) = selection() else { return };
    match crate::fs::trash::trash(&path) {
        Ok(n) => say(alloc::format!("moved to Trash as {}", n)),
        Err(e) => {
            refuse_notice(&e);
            say(alloc::format!("trash refused ({})", e));
        }
    }
    refresh(None);
}

fn do_show_trash() {
    let td = crate::fs::trash::trash_dir();
    if mt().stat(&td).is_err() {
        refuse_notice("the Trash is empty (nothing has been trashed yet)");
        return;
    }
    if let Some(m) = MODEL.lock().as_mut() {
        m.invalidate();
        m.show(&td);
        m.status = None;
        m.settle();
    }
}

/// Restore the selected Trash row to its original path (refused, with a NOTICE, if the folder is gone).
fn do_restore() {
    let Some((c, path, _)) = selection() else { return };
    if !starts_ci(&c, &crate::fs::trash::trash_dir()) {
        refuse_notice("Restore works on a row inside the Trash (use Show Trash)");
        return;
    }
    match crate::fs::trash::restore(&leaf(&path)) {
        Ok(p) => say(alloc::format!("restored {}", p)),
        Err(e) => {
            refuse_notice(&e);
            say(alloc::format!("restore refused ({})", e));
        }
    }
    refresh(None);
}

/// Two-step Empty: the first press arms (a NOTICE says so), a second within 5 s empties.
fn do_empty_trash() {
    if empty_armed() {
        EMPTY_ARMED.store(0, Ordering::Relaxed);
        match crate::fs::trash::empty() {
            Ok(n) => say(alloc::format!("emptied the Trash ({} items)", n)),
            Err(e) => {
                refuse_notice(&e);
                say(alloc::format!("empty refused ({})", e));
            }
        }
        refresh(None);
    } else {
        EMPTY_ARMED.store(now_s() + 1, Ordering::Relaxed);
        let msg = alloc::format!("Empty Trash again within {} s to delete {} items for good", EMPTY_WINDOW_S, crate::fs::trash::count());
        #[cfg(feature = "login")]
        crate::video::crystal::login::notice_show(b"Trash", msg.as_bytes());
        say(msg);
    }
}

fn do_copy() {
    let Some((_, path, _)) = selection() else { return };
    let r = ns_check(&path, true);
    log_op("copy", &path, "clipboard", &r);
    match r {
        Ok(()) => {
            *CLIP.lock() = Some(path.clone());
            say(alloc::format!("copied {}", leaf(&path)));
        }
        Err(why) => refuse_notice(&why),
    }
}

fn do_paste() {
    let Some(src) = CLIP.lock().clone() else {
        log_op("paste", "-", "-", &Err(String::from("clipboard-empty")));
        say(String::from("nothing to paste"));
        return;
    };
    let Some(c) = cwd() else { return };
    let r = op_copy(&src, &c);
    let (dst, res) = match r {
        Ok(d) => (d, Ok(())),
        Err(e) => (c.clone(), Err(e)),
    };
    log_op("paste", &src, &dst, &res);
    if let Err(e) = &res {
        if e.starts_with("files are") || e.starts_with("a path") {
            refuse_notice(e);
        }
        say(alloc::format!("paste refused ({})", e));
        refresh(None);
    } else {
        refresh(Some(&leaf(&dst)));
    }
}

fn do_info() {
    let Some((c, path, _)) = selection() else { return };
    super::getinfo::open(&path); // ATTRCOLUMNS (B402): Get Info — every attribute, typed, with its change time
    let t = mt();
    let (kind, size) = match t.stat(&path) {
        Ok(st) => (if matches!(st.kind, NodeKind::Dir) { "folder" } else { "file" }, st.size),
        Err(e) => {
            log_op("info", &path, "-", &Err(err_s(e)));
            return;
        }
    };
    let when = MODEL
        .lock()
        .as_ref()
        .and_then(|m| m.list.iter().find(|e| join(&c, &e.name) == path).map(|e| mtime_field(e.mtime.as_ref())))
        .unwrap_or_default();
    let owner = match t.volume_name(&path).as_deref() {
        Ok("native") => alloc::format!("{} (native ACL)", session_user().unwrap_or_else(|| String::from("public"))),
        Ok(_) => alloc::format!("{} (volume ACL, no per-file owner)", session_user().unwrap_or_else(|| String::from("volume"))),
        Err(_) => String::from("unknown"),
    };
    let l1 = alloc::format!("{} {} bytes, modified {}", kind, size, when.trim());
    let l2 = alloc::format!("owner: {}", owner);
    serial_println!("[quarry] op=info src={} dst=- ok=1 reason=- size={} owner={}", path, size, owner);
    let mut body = String::new();
    body.push_str(&l1);
    body.push('\n');
    body.push_str(&l2);
    let _ = body; // the inspector (getinfo) shows it; the NOTICE popup would cover it
    say(l1);
}

// ── keyboard ────────────────────────────────────────────────────────────────────────────────────

fn commit_edit() {
    let Some(e) = EDIT.lock().take() else { return };
    match e.target {
        Target::Attr(path, key, int) => {
            let who = session_user();
            let r = super::attrcols::commit(&path, &key, &e.buf, int, who.as_deref().unwrap_or(P));
            if let Err(why) = r {
                say(alloc::format!("{} not changed ({})", key, why));
            }
        }
        Target::Rename(src) => {
            let dst = join(&parent(&src), e.buf.trim());
            let r = op_rename(&src, &dst);
            log_op("rename", &src, &dst, &r);
            if let Err(why) = &r {
                if why.starts_with("files are") || why.starts_with("a path") {
                    refuse_notice(why);
                }
                say(alloc::format!("rename refused ({})", why));
                refresh(None);
            } else {
                refresh(Some(&leaf(&dst)));
            }
        }
        Target::NewFolder => {
            let c = cwd().unwrap_or_default();
            let dst = join(&c, e.buf.trim());
            let r = op_mkdir(&dst);
            log_op("mkdir", "-", &dst, &r);
            if let Err(why) = &r {
                if why.starts_with("files are") || why.starts_with("a path") {
                    refuse_notice(why);
                }
                say(alloc::format!("new folder refused ({})", why));
                refresh(None);
            } else {
                refresh(Some(&leaf(&dst)));
            }
        }
    }
}

/// Keys asked BEFORE `key_route`'s own table, with Quarry focused and on the glass. `true` = consumed.
/// While the edit field is up it takes everything (Enter commits, Esc cancels); otherwise Delete (0x7F),
/// `e` (rename — F2 is BRIGHTKEYS' and never arrives as a byte), `n` (new folder).
pub fn key_pre(c: u8) -> bool {
    if EDIT.lock().is_some() {
        match c {
            b'\r' | b'\n' => commit_edit(),
            0x1B => {
                *EDIT.lock() = None;
            }
            0x08 | 0x7F => {
                if let Some(e) = EDIT.lock().as_mut() {
                    e.buf.pop();
                }
            }
            0x20..=0x7E => {
                if let Some(e) = EDIT.lock().as_mut() {
                    if e.buf.len() < 100 {
                        e.buf.push(c as char);
                    }
                }
            }
            _ => {}
        }
        repaint();
        return true;
    }
    if c == 0x1B && (super::getinfo::is_up() || super::attrcols::menu_up()) {
        super::getinfo::dismiss();
        super::attrcols::menu_press_at(None);
        repaint();
        return true;
    }
    if MENU_UP.load(Ordering::Acquire) && c == 0x1B {
        *MENU.lock() = None;
        MENU_UP.store(false, Ordering::Release);
        repaint();
        return true;
    }
    let list_focus = MODEL.lock().as_ref().map(|m| m.focus == Pane::List).unwrap_or(false);
    if !list_focus {
        return false;
    }
    match c {
        0x7F if SHIFT_HELD.load(Ordering::Relaxed) => do_delete(),
        0x7F => do_trash(),
        b'D' => do_delete(),
        b'e' | b'E' => start_rename(),
        b'n' | b'N' => start_new_folder(),
        b'i' | b'I' => do_info(), // ATTRCOLUMNS (B402): Get Info
        _ => return false,
    }
    repaint();
    true
}

/// Cmd/Ctrl-C and -V (`Action::Copy` / `Action::Paste`) inside Quarry: the clipboard carries a PATH.
pub fn action(a: crate::video::keymap::Action) -> bool {
    use crate::video::keymap::Action;
    match a {
        Action::Copy => do_copy(),
        Action::Paste => do_paste(),
        Action::GetInfo => do_info(), // ATTRCOLUMNS (B402): Cmd-I
        _ => return false,
    }
    repaint();
    true
}

// ── the fixture ─────────────────────────────────────────────────────────────────────────────────

fn listed(t: &MountTable, dir: &str, name: &str) -> Option<u64> {
    t.read_dir(dir).ok()?.into_iter().find(|e| e.name.eq_ignore_ascii_case(name)).map(|e| e.size)
}

/// `:: QUARRYOPS: ops=[mkdir,rename,copy,delete] ok= refused= -> PASS ::` — runs the four ops on a
/// scratch folder under the user's home and VERIFIES BY LISTING; two refusals (outside the namespace,
/// the home itself) prove the DIRNS rule.
pub fn selftest() {
    let t = mt();
    let base = home_base();
    let scratch = join(&base, "QOPSTMP");
    let _ = delete_tree(&t, &scratch, 0);
    if !exists(&t, &base) {
        let _ = t.create(&base, NodeKind::Dir, P);
    }
    let (mut ok, mut refused) = (0u32, 0u32);
    // mkdir
    let sub = join(&scratch, "SUBDIR");
    let mk = op_mkdir(&scratch).and_then(|_| op_mkdir(&sub));
    log_op("mkdir", "-", &sub, &mk);
    if let Err(e) = &mk {
        serial_println!(":: QUARRYOPS: ops=[mkdir,rename,copy,delete] base={} reason={} -> SKIP ::", base, e);
        return;
    }
    if listed(&t, &scratch, "SUBDIR").is_some() {
        ok += 1;
    }
    // rename (long name: the LFN round trip)
    let a = join(&scratch, "A.TXT");
    let long = "quarry long name file.txt";
    let _ = t.create(&a, NodeKind::File, P).and_then(|_| t.write(&a, 0, b"hello", P));
    let dst = join(&scratch, long);
    let rn = op_rename(&a, &dst);
    log_op("rename", &a, &dst, &rn);
    let rn_ok = rn.is_ok() && listed(&t, &scratch, long) == Some(5) && listed(&t, &scratch, "A.TXT").is_none();
    if rn_ok {
        ok += 1;
    }
    // copy (paste into SUBDIR) and compare the bytes
    let cp = op_copy(&dst, &sub);
    log_op("copy", &dst, &sub, &cp.clone().map(|_| ()));
    let cp_ok = match &cp {
        Ok(p) => listed(&t, &sub, &leaf(p)) == Some(5) && t.read(p, 0, 16).map(|b| b == b"hello").unwrap_or(false),
        Err(_) => false,
    };
    if cp_ok {
        ok += 1;
    }
    // delete (tree) and confirm absence from the parent listing
    let dl = op_delete(&scratch);
    log_op("delete", &scratch, "-", &dl);
    if dl.is_ok() && listed(&t, &base, "QOPSTMP").is_none() {
        ok += 1;
    }
    // the refusals
    if ns_check("/boot/QOPS.TXT", true).is_err() {
        refused += 1;
    }
    if op_delete(&base).is_err() {
        refused += 1;
    }
    serial_println!("[quarryops] lfn_rename={} copy_bytes={}", rn_ok, cp_ok);
    let pass = ok == 4 && refused == 2;
    serial_println!(":: QUARRYOPS: ops=[mkdir,rename,copy,delete] ok={} refused={} -> {} ::", ok, refused, if pass { "PASS" } else { "FAIL" });
}

// ── ATTRCOLUMNS (B402): the edit field over an attribute cell ───────────────────────────────────

/// Open the edit field on an attribute cell (the caller may hold `MODEL`: this takes only the edit leaf).
pub(super) fn begin_attr_edit(path: String, key: String, buf: String, int: bool) {
    *EDIT.lock() = Some(Edit { buf, target: Target::Attr(path, key, int) });
}

/// A double-click cancels an attribute edit its first press opened.
pub(super) fn cancel_attr_edit() {
    let mut e = EDIT.lock();
    if matches!(e.as_ref().map(|x| &x.target), Some(Target::Attr(..))) {
        *e = None;
    }
}
