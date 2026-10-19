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
//! Lock discipline: UI state (MENU/EDIT) are leaf locks (the copied path is on video/clipboard.rs, SMALLFIX4 F13); the op bodies run with NO Quarry lock
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
    let _ = crate::video::dialog::notice(b"Quarry", why.as_bytes()); // DIALOG2 (B404): THE router — sorted error -> dialog, information -> toast
    let _ = why;
}

// ── UI state ────────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Item {
    Open,
    OpenWith,
    Rename,
    Delete,
    DeletePerm,
    NewFolder,
    Copy,
    Paste,
    Info,
    /// QUARRYVERSIONS (B732): 'Previous versions…' — the file's version ring (FILEVERSIONS B714) in the inspector.
    Versions,
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
            Item::OpenWith => "Open With...", // FILETYPES (B423)
            Item::Rename => "Rename",
            Item::Delete => "Move to Trash",
            Item::DeletePerm => "Delete Permanently",
            Item::NewFolder => "New Folder",
            Item::Copy => "Copy",
            Item::Paste => "Paste",
            Item::Info => "Get Info",
            Item::Versions => "Previous versions\u{2026}", // QUARRYVERSIONS (B732)
            Item::ShowTrash => "Show Trash",
            Item::Restore => "Restore",
            Item::EmptyTrash => "",
        })
    }
}

const ITEMS: [Item; 13] = [Item::Open, Item::OpenWith, Item::Rename, Item::Delete, Item::DeletePerm, Item::NewFolder, Item::Copy, Item::Paste, Item::Info, Item::Versions, Item::ShowTrash, Item::Restore, Item::EmptyTrash];

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
static MENU: crate::sync::Mutex<Option<Menu>> = crate::sync::Mutex::new(None);
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
static EDIT: crate::sync::Mutex<Option<Edit>> = crate::sync::Mutex::new(None);

/// MENUS2 (B652): the OUTER sizes — the sheet's body (the widest label, `Empty Trash (NN items)`, at `INSET + PAD`
/// from each side, inside the item pill's inset) plus `sheet::BAND()` on the right and at the foot for the `elev.2`
/// shadow (MENUSCRAFT §2). The height carries the container's `DROP_PADY` above and below the items.
fn menu_w(g: &Geom) -> usize {
    use crate::video::crystal::sheet;
    22 * g.cell_w() + 2 * PAD() + 2 * sheet::INSET() + sheet::BAND()
}
fn menu_h(g: &Geom) -> usize {
    ITEMS.len() * g.row_h() + 2 * crate::video::crystal::DROP_PADY() + crate::video::crystal::sheet::BAND()
}

/// MENUS2 (B652) — the body of the menu at `(x, y)` (source px): `(bw, bh, pady)`. The band is outside it.
fn menu_body(g: &Geom) -> (usize, usize, usize) {
    let band = crate::video::crystal::sheet::BAND();
    (menu_w(g) - band, menu_h(g) - band, crate::video::crystal::DROP_PADY())
}

/// MENUS2 (B652) — the item under the pointer, or `None`: the system cursor read through the input path's one door
/// (`panel_info_nonblocking`, LOCKFIX), mapped by [`to_source`] (wm's `hit_test`/`info`, no Quarry lock taken), and
/// tested against the BODY less its padding — the band and the padding light nothing.
fn menu_hover(g: &Geom, x: usize, y: usize) -> Option<usize> {
    let i = crate::video::panel_info_nonblocking()?;
    let (px, py) = crate::pal::cursor::pos(i.width as i32, i.height as i32);
    let (sx, sy) = to_source(px, py)?;
    let (bw, _, pady) = menu_body(g);
    if sx < x || sx >= x + bw || sy < y + pady {
        return None;
    }
    let k = (sy - y - pady) / g.row_h();
    if k < ITEMS.len() { Some(k) } else { None }
}

/// MENUS2 (B652) — **Quarry's file-ops menu on the sheet** (MENUSCRAFT §9), painted IN Quarry's own surface `px`
/// (source px, stride `g.w`) at `(x, y)` through the kit (`video::draw`): the `elev.2` lift as two `shadow_rrect`
/// layers in `theme::shadow_soft()` (the mapped `elev.2` ink for an overlay outside crystal, MENUSCRAFT §7's toast ×
/// row; the tight layer at two thirds of it, elev.2's own tight:wide ratio), the `surface.overlay` body at
/// `radius.overlay` with all four corners round (a context menu floats), the `line.subtle` hairline, the hovered
/// item's `sheet::pill`, the labels in `text.primary` (`on_accent` on the pill), and `sheet::seal_row` over every
/// row of the outer rect (no pure white: MENU-UNDER). `sheet::row` is not used: its `Under` is a scene ESTIMATE in
/// panel coordinates, and here the pixels beneath the band are Quarry's own, already in `px` — the shadow blends
/// over the TRUE scene (MENUSCRAFT §4's exactness, where the owner holds the pixels).
fn menu_paint(px: &mut [u32], g: &Geom, x: usize, y: usize, hover: Option<usize>) {
    use crate::video::{crystal::sheet, draw};
    let (w, h) = (menu_w(g), menu_h(g));
    let (bw, bh, pady) = menu_body(g);
    let rh = g.row_h();
    if x + w > g.w || y + h > g.h || px.len() < g.w * g.h || bw <= 2 * sheet::INSET() {
        return;
    }
    let rad = sheet::RADIUS().min(bw as i32 / 2).min(bh as i32 / 2).max(0);
    {
        let mut s = draw::Surf::new(&mut px[..g.w * g.h], g.w);
        let b = draw::Rect::new(x as i32, y as i32, bw as i32, bh as i32);
        let band = sheet::BAND() as i32;
        let drop = (crate::ui::px(3) as i32).min(band - 1).max(0);
        let wide = theme::shadow_soft();
        let tight = ((wide >> 24) * 2 / 3) << 24;
        draw::shadow_rrect(&mut s, b.offset(0, drop), rad, (band - drop).max(1), wide);
        draw::shadow_rrect(&mut s, b, rad, (crate::ui::px(2) as i32).min(band).max(1), tight);
        draw::fill_rrect(&mut s, b, rad, sheet::surface_overlay());
        draw::stroke_rrect(&mut s, b, rad, 1, draw::Ink::argb(sheet::line_subtle()));
    }
    for (i, it) in ITEMS.iter().enumerate() {
        let top = y + pady + i * rh;
        let hot = hover == Some(i);
        if hot {
            for r in top..top + rh {
                sheet::pill(&mut px[r * g.w + x..r * g.w + x + bw], bw, r - top, 0, rh);
            }
        }
        let ink = if hot { sheet::on_accent() } else { sheet::text_primary() };
        text(px, g, x + sheet::INSET() + PAD(), top + g.ts, it.label().as_bytes(), x + bw - sheet::INSET(), ink);
    }
    for r in y..y + h {
        sheet::seal_row(&mut px[r * g.w + x..r * g.w + x + w]);
    }
}

/// MENUS2 (B652) — `tests menuscraft`'s `quarry_menu=` and `band_exact=` legs: `(menu ok, band exact)`. The live
/// painter ([`menu_paint`]) over a pure geometry (`geometry(1280, 800)`), item 0 hovered, into two surfaces of
/// different fills (`surface.content`, then the accent). Menu ok: the body's centre is `surface.overlay`, the pill
/// sits inside the inset (accent at the body's right inner edge, the face outside the inset), no pure white in the
/// outer rect. Band exact: the band pixel beside the body is darker than its own fill in BOTH surfaces and differs
/// between them — the shadow blended over the true pixels beneath, not over an estimate.
pub fn sheet_probe() -> (bool, bool) {
    use crate::video::crystal::sheet;
    let Some(g) = geometry(1280, 800) else { return (false, false) };
    let (w, h) = (menu_w(&g), menu_h(&g));
    let (bw, bh, pady) = menu_body(&g);
    if w > g.w || h > g.h {
        return (false, false);
    }
    let fills = [theme::surface_content() & 0x00FF_FFFF, theme::accent() & 0x00FF_FFFF];
    let luma = |c: u32| (((c >> 16) & 255) * 77 + ((c >> 8) & 255) * 150 + (c & 255) * 29) >> 8;
    let (mut menu_ok, mut darker) = (true, true);
    let mut band_px = [0u32; 2];
    for (k, &f) in fills.iter().enumerate() {
        let mut px = alloc::vec![f; g.w * g.h];
        menu_paint(&mut px, &g, 0, 0, Some(0));
        let at = |x: usize, y: usize| px[y * g.w + x] & 0x00FF_FFFF;
        menu_ok &= at(bw / 2, bh / 2) == sheet::surface_overlay();
        menu_ok &= at(bw - sheet::INSET() - 2, pady + g.row_h() / 2) == sheet::accent_fill() && at(sheet::INSET() / 2, pady + g.row_h() / 2) != sheet::accent_fill();
        for r in 0..h {
            menu_ok &= !px[r * g.w..r * g.w + w].iter().any(|&c| c & 0x00FF_FFFF == theme::pure_white());
        }
        band_px[k] = at(bw + 1, bh / 2);
        darker &= luma(band_px[k]) < luma(f);
    }
    (menu_ok, darker && band_px[0] != band_px[1])
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
        fill(px, g, x, y, w, rh, crate::video::theme::surface_content());
        keyline(px, g, Rect { x, y, w, h: rh }, theme::accent());
        let mut s: Vec<u8> = Vec::new();
        s.extend_from_slice(label.as_bytes());
        s.extend_from_slice(e.buf.as_bytes());
        s.push(b'_');
        text(px, g, x + PAD(), y + g.ts, &s, x + w, theme::text_primary());
    }
    let at = MENU.lock().as_ref().map(|mn| (mn.x, mn.y)); // MENUS2 (B652): the origin copied out, so the hover read below holds no Quarry UI lock
    if let Some((mx, my)) = at {
        menu_paint(px, g, mx, my, menu_hover(g, mx, my)); // MENUS2 (B652): the sheet (was a raised face, a keyline and flat rows)
    }
    super::getinfo::paint(m, px); // ATTRCOLUMNS (B402): the inspector
    super::openwith::paint(m, px); // FILETYPES (B423): Open With…
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
        let cols_hit = super::toolbar::view() == super::toolbar::View::Columns && super::millercols::right_select(m, sx, sy); // SMALLFIX4 (COLUMNSVIEW owed): the Columns view's panes, not the list geometry, take the right press
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
        if cols_hit {
            // SMALLFIX4: the pane under the pointer already took the selection (above, before `g` borrows the model).
        } else if sx >= li.x && sx < li.x + li.w && sy >= body_y {
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
    if super::openwith::is_up() {
        // FILETYPES (B423): a press while Open With… is up picks a registrant or dismisses the list.
        let hit = to_source(x, y);
        let act = match MODEL.lock().as_ref() {
            Some(m) => super::openwith::press(hit, &m.geom),
            None => { super::openwith::dismiss(); Act::None }
        };
        run_act(act);
        repaint();
        return hit.is_some();
    }
    if super::getinfo::is_up() {
        // ATTRCOLUMNS (B402): any press dismisses the inspector; one inside Quarry is consumed by it.
        // QUARRYVERSIONS (B732): except a press on a version row's Restore or Open, which runs it (the hit is read
        // under MODEL then the panel leaf, `paint`'s order; the verb runs with neither held).
        let hit = to_source(x, y);
        let pick = match (hit, MODEL.lock().as_ref()) {
            (Some(h), Some(m)) => super::getinfo::version_hit(&m.geom, h),
            _ => None,
        };
        match pick {
            Some((path, seq, restore)) => version_press(&path, seq, restore),
            None => super::getinfo::dismiss(),
        }
        repaint();
        return hit.is_some();
    }
    if !MENU_UP.load(Ordering::Acquire) {
        return false;
    }
    let mn = MENU.lock().take();
    MENU_UP.store(false, Ordering::Release);
    let Some(mn) = mn else { return false };
    let hit = to_source(x, y);
    let mut picked = None;
    let origin = (mn.x, mn.y);
    if let Some((sx, sy)) = hit {
        let g = match MODEL.lock().as_ref() {
            Some(m) => { let (bw, _, pady) = menu_body(&m.geom); (bw, m.geom.row_h(), pady) } // MENUS2 (B652): the BODY less its padding — a press on the band dismisses (MENUSCRAFT §2)
            None => return true,
        };
        if sx >= mn.x && sx < mn.x + g.0 && sy >= mn.y + g.2 {
            let i = (sy - mn.y - g.2) / g.1;
            if i < ITEMS.len() {
                picked = Some(ITEMS[i]);
            }
        }
    }
    match picked {
        Some(Item::OpenWith) => {
            if let Some((_, path, false)) = selection() { super::openwith::open(&path, origin.0, origin.1); } // FILETYPES (B423)
            repaint();
        }
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
        Item::Versions => do_versions(), // QUARRYVERSIONS (B732)
        Item::OpenWith => {} // FILETYPES (B423): taken in `menu_press` (it needs the menu's origin)
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
    match crate::video::quarry::undo::trash(&path) {
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
    match crate::video::quarry::undo::restore(&leaf(&path)) {
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
        let _ = crate::video::dialog::notice(b"Trash", msg.as_bytes()); // DIALOG2 (B404): THE router — sorted error -> dialog, information -> toast
        say(msg);
    }
}

fn do_copy() {
    let Some((_, path, _)) = selection() else { return };
    let r = ns_check(&path, true);
    log_op("copy", &path, "clipboard", &r);
    match r {
        Ok(()) if crate::video::clipboard::set_file_ref(&path) => { // SMALLFIX4 (F13): the ONE clipboard, typed file-ref
            say(alloc::format!("copied {}", leaf(&path)));
        }
        Ok(()) => {
            log_op("copy", &path, "clipboard", &Err(String::from("clipboard-refused")));
            say(String::from("copy refused (clipboard)"));
        }
        Err(why) => refuse_notice(&why),
    }
}

fn do_paste() {
    let Some(src) = crate::video::clipboard::get_file_ref() else { // SMALLFIX4 (F13)
        log_op("paste", "-", "-", &Err(String::from("clipboard-empty")));
        say(String::from("nothing to paste"));
        return;
    };
    let Some(c) = cwd() else { return };
    let r = crate::video::quarry::undo::copy(&src, &c);
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
    // SETTINGSFILES (B407, R98): a settings file's Show Info lists its keys and when it auto-saved (ATTRCOLUMNS'
    // Get Info inspector joins `settingsfiles::info_lines` at the merge).
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    for l in crate::video::settingsfiles::info_lines(&path) {
        body.push('\n');
        body.push_str(&l);
    }
    #[cfg(feature = "login")]
    let _ = crate::video::dialog::notice(leaf(&path).as_bytes(), body.as_bytes()); // DIALOG2 (B404): THE router — sorted error -> dialog, information -> toast
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
            let r = crate::video::quarry::undo::rename(&src, &dst);
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
            let r = crate::video::quarry::undo::mkdir(&dst);
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
        Action::GetInfo => do_info(), Action::Undo => { let _ = crate::video::quarry::undo::undo(); } Action::Redo => { let _ = crate::video::quarry::undo::redo(); } // ATTRCOLUMNS (B402): Cmd-I. UNDOKEYS (B743): ⌘Z / ⌘⇧Z, the journal's two verbs (the Edit menu's rows call the same pair)
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
    if ns_check("/var/tmp/QOPS.TXT", true).is_err() { // SMALLFIX3 (B416): outside the home, off the sacred boot FAT (R99) — a namespace refusal, never a write
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

/// QUARRYLIVE (B494): the folder every op is confined to, for `tests quarrylive`'s scratch folder.
pub(super) fn home_dir() -> String {
    home_base()
}

// KEYCHARS (rmbp-ledger B718) — TAIL-APPENDED. The edit field takes a layout CHARACTER (`pal::Event::Char`, at or above
// U+0080) the way `key_pre` takes a printable byte: into the open field, the same 100-byte bound, then a repaint.

/// One typed character into the open edit field (rename, new folder, attribute cell); `false` when no field is open
/// (the character is not Quarry's). The body `tests keychars` drives is [`edit_char`].
pub fn key_char(ch: char) -> bool {
    let took = edit_char(ch);
    if took {
        repaint();
    }
    took
}

/// [`key_char`] without the repaint: push `ch` into the open field.
fn edit_char(ch: char) -> bool {
    match EDIT.lock().as_mut() {
        Some(e) => {
            if e.buf.len() + ch.len_utf8() <= 100 {
                e.buf.push(ch);
            }
            true
        }
        None => false,
    }
}

/// KEYCHARS fixture: type `chars` into a SCRATCH rename field through [`edit_char`] and hand back what it holds; the
/// field is closed again. `None` while a person has a field open (it is theirs; nothing is touched).
pub fn key_char_fixture(chars: &[char]) -> Option<String> {
    {
        let mut g = EDIT.lock();
        if g.is_some() {
            return None;
        }
        *g = Some(Edit { buf: String::new(), target: Target::Rename(String::from("/keychars")) });
    }
    for &c in chars {
        let _ = edit_char(c);
    }
    EDIT.lock().take().map(|e| e.buf)
}

// ── QUARRYVERSIONS (rmbp-ledger B732): 'Previous versions…' — the face of FILEVERSIONS (B714) ──────────────────────
//
// BP-bar-14 / BP-storage-24: an overwrite is never the only copy, and the user reaches the copies from the file. The
// menu row (`Item::Versions`, after Get Info) opens the inspector (`getinfo::open_versions`) on one row per version,
// newest first: the date it was replaced (`clock::locale::fmt_stamp`), its size, Restore and Open. A folder has no
// list (the row says so and opens nothing); a file with none says 'No previous versions'.
//
// * The path: Quarry holds MOUNT-TABLE paths; the three seams (`fs::unafs::{versions, read_version,
//   restore_version}`) take the NATIVE volume's. [`native_path`] asks the table which backend claims the path (the
//   ROOTDISK alias `/volumes/UnaOS/…` included) and lifts the remainder to the volume through the backend's own
//   `on_volume` — the one mount-space → volume-space definition; a non-native volume (FAT, a stick) keeps none.
// * Restore ([`op_restore_version`]) names the version by its `seq` (the ring's index shifts under a later write),
//   is confined to the home (DIRNS, as every op), and is ITSELF a versioned write — the crate keeps the current
//   contents as the newest version — so it is undone by restoring that version: no undo-journal entry is pushed
//   (UNDOMOVE's journal is the namespace's; this changes no name). It bypasses the VFS write dispatch, so it bumps
//   the namespace generation itself (the size a listing prints changed), refreshes the folder, re-lists, and toasts
//   'Restored the version from <date>'.
// * Open ([`op_open_version`]) reads the version whole and writes it to `<home>/.Trash/.versions/<stem> (version
//   <seq>)<.ext>` — the extension kept so the type and opener route as the file's — then hands that path to the ONE
//   opener dispatch. Why there: BP-bar-14 allows no hidden folder under the home but `.Trash`; a copy is not the
//   user's document, so it must not land in the folder they are looking at; and Empty Trash removes it (trash_core
//   lists only stamped/indexed objects, so the scratch never shows as a Trash item, but `empty` deletes everything).
//
// Wire: `[quarry] versions path=<p> native=<p|-> listed=<n>`, `[quarry] op=version-restore src=<p> dst=seq<k> ok=
// reason=`, `[quarry] version restored path=<p> seq=<k> from=<date>`, `[quarry] op=version-open src=<p> dst=<tmp> ok=
// reason=`.

/// QUARRYVERSIONS (B732): the native volume's path for the mount-table `path`, or `None` when another volume claims it.
pub fn native_path(path: &str) -> Option<String> {
    let t = mt();
    let (b, rel) = t.resolve(path).ok()?;
    if b.volume_name() != "native" {
        return None;
    }
    let v = b.on_volume(rel);
    Some(if v.is_empty() { String::from("/") } else if v.starts_with('/') { v } else { alloc::format!("/{}", v) })
}

#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
fn ver_list(np: &str) -> Option<Vec<(u64, u64, u64)>> {
    crate::fs::unafs::versions(np).map(|v| v.iter().map(|i| (i.seq, i.mtime, i.size)).collect())
}
#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
fn ver_read(np: &str, n: usize) -> Option<Vec<u8>> {
    crate::fs::unafs::read_version(np, n)
}
#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
fn ver_restore(np: &str, n: usize) -> bool {
    crate::fs::unafs::restore_version(np, n)
}
#[cfg(not(any(target_arch = "aarch64", feature = "unafs")))]
fn ver_list(_: &str) -> Option<Vec<(u64, u64, u64)>> {
    None
}
#[cfg(not(any(target_arch = "aarch64", feature = "unafs")))]
fn ver_read(_: &str, _: usize) -> Option<Vec<u8>> {
    None
}
#[cfg(not(any(target_arch = "aarch64", feature = "unafs")))]
fn ver_restore(_: &str, _: usize) -> bool {
    false
}

/// QUARRYVERSIONS (B732): the versions of the file at `path`, newest first, as `(seq, replaced-at unix s, size)`.
pub fn version_rows(path: &str) -> Result<Vec<(u64, u64, u64)>, &'static str> {
    let np = native_path(path).ok_or("this volume keeps none")?;
    ver_list(&np).ok_or("unreadable")
}

/// The ring index of the version stamped `seq` and its replaced-at time.
fn version_at(np: &str, seq: u64) -> Result<(usize, u64), String> {
    let rows = ver_list(np).ok_or_else(|| String::from("the versions are unreadable"))?;
    rows.iter().position(|r| r.0 == seq).map(|n| (n, rows[n].1)).ok_or_else(|| String::from("that version is no longer kept"))
}

/// The date a person reads for a version.
pub fn version_date(unix: u64) -> String {
    if unix == 0 { String::from("an unknown date") } else { crate::clock::locale::fmt_stamp(crate::clock::locale::Painter::Quarry, unix) }
}

/// QUARRYVERSIONS (B732): make the version stamped `seq` the contents of `path` again. Returns its replaced-at time.
pub fn op_restore_version(path: &str, seq: u64) -> Result<u64, String> {
    ns_check(path, false)?;
    let np = native_path(path).ok_or_else(|| String::from("this volume keeps no versions"))?;
    let (n, when) = version_at(&np, seq)?;
    if !ver_restore(&np, n) {
        return Err(String::from("the file system refused the restore"));
    }
    let _ = crate::fs::ns_bump(Ok::<(), ()>(())); // the bytes changed behind the VFS write dispatch: a listing change
    Ok(when)
}

/// QUARRYVERSIONS (B732): the scratch folder an opened version is written to (BP-bar-14: under `.Trash`, the one hidden folder).
pub fn versions_scratch() -> String {
    join(&crate::fs::trash::trash_dir(), ".versions")
}

/// QUARRYVERSIONS (B732): write the version stamped `seq` of `path` to the scratch folder; returns the copy's path.
pub fn op_open_version(path: &str, seq: u64) -> Result<String, String> {
    ns_check(path, false)?;
    let np = native_path(path).ok_or_else(|| String::from("this volume keeps no versions"))?;
    let (n, _) = version_at(&np, seq)?;
    let bytes = ver_read(&np, n).ok_or_else(|| String::from("the version is unreadable"))?;
    let t = mt();
    let (td, dir) = (crate::fs::trash::trash_dir(), versions_scratch());
    for d in [&td, &dir] {
        if !exists(&t, d) {
            t.create(d, NodeKind::Dir, P).map_err(err_s)?;
        }
    }
    let name = leaf(path);
    let copy = match name.rfind('.') {
        Some(i) if i > 0 => alloc::format!("{} (version {}){}", &name[..i], seq, &name[i..]),
        _ => alloc::format!("{} (version {})", name, seq),
    };
    let dst = join(&dir, &copy);
    if exists(&t, &dst) {
        t.unlink(&dst, P).map_err(err_s)?;
    }
    t.create(&dst, NodeKind::File, P).map_err(err_s)?;
    let mut off = 0usize;
    while off < bytes.len() {
        let end = (off + COPY_CHUNK).min(bytes.len());
        t.write(&dst, off as u64, &bytes[off..end], P).map_err(err_s)?;
        off = end;
    }
    Ok(dst)
}

/// The menu row: open the version list for the selected FILE (a folder has none: M2).
fn do_versions() {
    let Some((_, path, is_dir)) = selection() else { return };
    if is_dir {
        log_op("versions", &path, "-", &Err(String::from("folder")));
        say(String::from("a folder keeps no previous versions"));
        return;
    }
    super::getinfo::open_versions(&path);
}

/// QUARRYVERSIONS (B732): a version row's button — what the inspector's press runs (and `tests fileversions` drives).
/// Restore: the restore, the folder refreshed, the list re-read, the toast. Open: the copy, then the opener.
pub fn version_press(path: &str, seq: u64, restore: bool) {
    let tag = alloc::format!("seq{}", seq);
    if restore {
        let r = op_restore_version(path, seq);
        log_op("version-restore", path, &tag, &r.clone().map(|_| ()));
        match r {
            Ok(when) => {
                let date = version_date(when);
                let title = alloc::format!("Restored the version from {}", date);
                serial_println!("[quarry] version restored path={} seq={} from={}", path, seq, date);
                let _ = crate::video::notify::post_full(b"Quarry", title.as_bytes(), leaf(path).as_bytes(), b"", crate::video::notify::ACT_NONE, &[]);
                refresh(Some(&leaf(path)));
                say(title);
                super::getinfo::open_versions(path);
            }
            Err(e) => {
                refuse_notice(&e);
                say(alloc::format!("restore refused ({})", e));
            }
        }
    } else {
        let r = op_open_version(path, seq);
        log_op("version-open", path, r.as_deref().unwrap_or("-"), &r.clone().map(|_| ()));
        super::getinfo::dismiss();
        match r {
            Ok(copy) => {
                let (mime, opener) = route(&mt(), &copy);
                run_act(super::openers::act(opener, copy, mime));
            }
            Err(e) => {
                refuse_notice(&e);
                say(alloc::format!("open refused ({})", e));
            }
        }
    }
}

/// QUARRYVERSIONS (B732): `tests fileversions`' `menu=` leg — the row is in the menu, right after Get Info, and says so.
pub fn versions_item_probe() -> bool {
    let at = ITEMS.iter().position(|&i| i == Item::Versions);
    let info = ITEMS.iter().position(|&i| i == Item::Info);
    matches!((at, info), (Some(a), Some(i)) if a == i + 1) && Item::Versions.label() == "Previous versions\u{2026}"
}
