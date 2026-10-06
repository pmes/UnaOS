// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! TEXTEDIT — a text EDITOR window, built on FILEVIEW's pattern (`video/fileview.rs`).
//!
//! `edit <path>` (a new or existing file, at most [`MAX_BYTES`]) and Quarry's double-click on a text
//! file the user OWNS (`/home/<user>/…`; any other text file still opens the read-only viewer) open a
//! window with a caret. Insert / backspace / enter, Up/Down by key, Left/Right/Home/End and the
//! `Shift` selection moves by the resolved desktop ACTIONS (`pal::Event::Action` — the arrow BYTE is
//! also typed, so the byte is ignored here and the action is the one authority), click places the
//! caret, `⌘/Ctrl-C/X/V/A` through the clipboard seam (`video::clipboard::{set,get}`), `Ctrl-S`
//! (`0x13`) saves through the mount table the way `write` does (unlink + create + write).
//!
//! Text is bytes: printable ASCII only; on load `\t` becomes four spaces, `\r` is dropped, a file
//! with NUL or a byte >= 0x80 is REFUSED (the face has no glyph and a save would corrupt it).
//!
//! Close prints `[edit] closed dirty=<0|1>`. DIALOG2 (B404): a DIRTY close by the close box ASKS — a sheet on the
//! window, "Do you want to save the changes…" (Don't Save · Cancel · Save) — and the dirty state is declared to
//! `dialog::unsaved_declare` on every flip, so Log Out's confirm lists it.
//! The dirty mark in the title is `*` (the face carries no `•` glyph).
//!
//! Witness: `:: TEXTEDIT: path=<p> bytes=<n> lines=<n> edits=<n> saved=<n> -> PASS ::`.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::video::keymap::Action;
use crate::video::{theme, wm};

/// Kernel-furniture owner slot (`+ 6`, after FILEVIEW's `+ 5`).
pub const OWNER: u64 = wm::KERNEL_OWNER_BASE + 6;
const _: () = assert!(OWNER != wm::KERNEL_OWNER_CONSOLE && OWNER != wm::KERNEL_OWNER_DESKTOP);
const _: () = assert!(OWNER != super::fileview::OWNER);
#[cfg(feature = "quarry")] const _: () = assert!(OWNER != super::quarry::live::OWNER);

pub const MAX_BYTES: usize = 256 * 1024;
const CHUNK: usize = 16 * 1024;
#[allow(non_snake_case)] #[inline] fn WIN_W() -> usize { crate::ui::px(720) } // UIMETRICS (B372): a NATIVE window — physical px at the panel's dpi scale, drawn at scale 1
#[allow(non_snake_case)] #[inline] fn WIN_H() -> usize { crate::ui::px(480) }
#[allow(non_snake_case)] #[inline] fn PAD() -> usize { crate::ui::px(6) }
const WHEEL_ROWS: usize = 3;

static WIN: AtomicU32 = AtomicU32::new(wm::WIN_NONE);
static PENDING: spin::Mutex<Option<String>> = spin::Mutex::new(None);
static STATE: spin::Mutex<Option<State>> = spin::Mutex::new(None);

struct State {
    path: String,
    text: Vec<u8>,
    caret: usize,
    anchor: Option<usize>,
    want_col: Option<usize>,
    rows: Vec<(u32, u32)>,
    top: usize,
    vis: usize,
    cols: usize,
    w: usize,
    h: usize,
    surf: Vec<u32>,
    dirty: bool,
    title_dirty: bool,
    edits: usize,
}

fn title_of(path: &str, dirty: bool) -> String {
    let leaf = path.rsplit('/').next().unwrap_or(path);
    let mut t = String::new();
    if dirty {
        t.push('*');
    }
    t.push_str(leaf);
    t.truncate(wm::MAX_TITLE);
    t
}

/// The session user's home prefix `/home/<name>/`, or `None` (no login feature / no session).
fn home_prefix() -> Option<String> {
    #[cfg(feature = "login")]
    {
        let mut b = [0u8; crate::fs::users::NAME_MAX];
        let n = crate::fs::users::whoami(&mut b)?;
        let name = core::str::from_utf8(&b[..n]).ok()?;
        Some(alloc::format!("/home/{}/", name))
    }
    #[cfg(not(feature = "login"))]
    {
        None
    }
}

/// Does the session user own `path` — i.e. is it under `/home/<user>/` (the DIRNS namespace rule)?
/// An image with no user store treats any `/home/` path as owned. `..` components are refused. Pure
/// apart from the session read.
pub fn may_edit(path: &str) -> bool {
    if path.split('/').any(|c| c == "..") {
        return false;
    }
    let pre = home_prefix().unwrap_or_else(|| String::from("/home/"));
    path.len() > pre.len() && path.as_bytes()[..pre.len()].eq_ignore_ascii_case(pre.as_bytes())
}

pub fn is_open() -> bool {
    WIN.load(Ordering::Relaxed) != wm::WIN_NONE
}

/// Latch a path for [`service`] (click-router safe).
pub fn request_open(path: &str) {
    *PENDING.lock() = Some(String::from(path));
}

/// Drain the latch. Chained from `quarry::live::service`.
pub fn service() {
    let want = PENDING.lock().take();
    if let Some(p) = want {
        match open(&p) {
            Ok(_) => serial_println!("[quarry] open TEXT consumed=editor path={}", p),
            Err(e) => {
                serial_println!("[quarry] open TEXT consumed=refused path={} reason={}", p, e);
                serial_println!("[edit] refuse path={} reason={}", p, e);
            }
        }
    }
}

fn clean(raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut o = Vec::with_capacity(raw.len());
    for &b in raw {
        match b {
            b'\n' => o.push(b'\n'),
            b'\r' => {}
            b'\t' => o.extend_from_slice(b"    "),
            0x20..=0x7e => o.push(b),
            _ => return Err(String::from("not a text file (binary bytes)")),
        }
    }
    Ok(o)
}

fn read_file(path: &str) -> Result<Vec<u8>, String> {
    use crate::fs::vfs::{NodeKind, VfsError};
    let mt = crate::shell::vfs_mount_table();
    let st = match mt.stat(path) {
        Ok(s) => s,
        Err(VfsError::NoSuchPath) => return Ok(Vec::new()), // a NEW file: the first save creates it
        Err(e) => return Err(alloc::format!("vfs: {:?}", e)),
    };
    if matches!(st.kind, NodeKind::Dir) {
        return Err(String::from("is a directory"));
    }
    if st.size as usize > MAX_BYTES {
        return Err(String::from("file over the 256 KB editor cap"));
    }
    let want = st.size as usize;
    let mut out: Vec<u8> = Vec::new();
    if out.try_reserve_exact(want).is_err() {
        return Err(String::from("out of memory"));
    }
    while out.len() < want {
        let n = core::cmp::min(CHUNK, want - out.len());
        let got = mt.read(path, out.len() as u64, n).map_err(|e| alloc::format!("vfs: {:?}", e))?;
        if got.is_empty() {
            break;
        }
        out.extend_from_slice(&got);
    }
    Ok(out)
}

/// Rows for `text`: FILEVIEW's layout plus the empty last line a trailing `\n` (or no text) opens.
fn layout_rows(text: &[u8], cols: usize) -> Vec<(u32, u32)> {
    let mut rows = super::fileview::layout(text, cols).rows;
    if text.is_empty() || *text.last().unwrap() == b'\n' {
        rows.push((text.len() as u32, text.len() as u32));
    }
    rows
}

/// Index of the row holding byte position `pos` (the last row starting at or before it). Pure.
fn row_of(rows: &[(u32, u32)], pos: usize) -> usize {
    let mut r = 0;
    for (i, &(a, _)) in rows.iter().enumerate() {
        if (a as usize) <= pos {
            r = i;
        } else {
            break;
        }
    }
    r
}

/// **Open `path` in an editor window** (a missing path is a new empty buffer). `(bytes, lines)`.
pub fn open(path: &str) -> Result<(usize, usize), String> {
    let raw = read_file(path)?;
    let text = clean(&raw)?;
    let pi = crate::video::panel_info_nonblocking().ok_or_else(|| String::from("panel busy"))?;
    let (pw, ph) = (pi.width, pi.height);
    let w = WIN_W().min(pw.saturating_sub(2 * wm::BORDER()).max(1));
    let h = WIN_H().min(ph.saturating_sub(wm::TITLE_H() + 2 * wm::BORDER()).max(1));
    let face = super::text::Face::Grid; // UIMETRICS: the dpi-sized mono grid face
    let (cw, ch) = (face.cell_w(), face.cell_h());
    let cols = w.saturating_sub(2 * PAD() + crate::ui::px(6)) / cw;
    let vis = h.saturating_sub(2 * PAD()) / ch;
    if cols < 8 || vis < 2 {
        return Err(String::from("window below floor"));
    }
    let len = w * h;
    let mut surf: Vec<u32> = Vec::new();
    if surf.try_reserve_exact(len).is_err() {
        return Err(String::from("out of memory"));
    }
    surf.resize(len, theme::content_fill());
    if is_open() {
        close();
    }
    let (_s, ow, oh) = wm::spawn_geometry_native(w, h).ok_or_else(|| String::from("geometry unavailable"))?;
    let wtop = crate::ui_status::top_chrome_h(pw, ph);
    let ox = pw.saturating_sub(ow) / 2;
    let oy = wtop + ph.saturating_sub(wtop).saturating_sub(crate::ui_status::chrome_h(ph)).saturating_sub(oh) / 2;
    let rows = layout_rows(&text, cols);
    let n_bytes = text.len();
    let n_lines = text.iter().filter(|&&b| b == b'\n').count() + (text.last().map_or(0, |&b| (b != b'\n') as usize));
    let mut st = State {
        path: String::from(path), text, caret: 0, anchor: None, want_col: None, rows, top: 0, vis, cols, w, h, surf,
        dirty: false, title_dirty: false, edits: 0,
    };
    paint(&mut st);
    let base = st.surf.as_ptr() as usize;
    let title = title_of(path, false);
    let id = wm::create_at_native(OWNER, base, len * 4, w as u32, h as u32, (w * 4) as u32, title.as_bytes(), ox + wm::BORDER(), oy + wm::TITLE_H() + wm::BORDER());
    if id == wm::WIN_NONE {
        return Err(String::from("window create failed"));
    }
    *STATE.lock() = Some(st);
    WIN.store(id, Ordering::Relaxed);
    wm::winid_register_holder(&WIN, "textedit");
    wm::focus_changed(OWNER);
    let _ = wm::present(id);
    serial_println!("[edit] open win={} path={} bytes={} lines={} cols={} vis={}", id, path, n_bytes, n_lines, cols, vis);
    Ok((n_bytes, n_lines))
}

/// Close the window. Asks nothing; prints whether work was lost.
pub fn close() {
    let id = WIN.swap(wm::WIN_NONE, Ordering::Relaxed);
    if id == wm::WIN_NONE {
        return;
    }
    let (dirty, path, edits) = STATE.lock().as_ref().map(|s| (s.dirty, s.path.clone(), s.edits)).unwrap_or((false, String::new(), 0));
    wm::close(id);
    *STATE.lock() = None;
    crate::video::dialog::unsaved_declare(OWNER, b"", false); // DIALOG2: nothing of ours is unsaved any more
    serial_println!("[edit] closed dirty={} path={} edits={} win={}", dirty as u8, path, edits, id);
}

fn fill(st: &mut State, x: usize, y: usize, rw: usize, rh: usize, c: u32) {
    let (w, h) = (st.w, st.h);
    for yy in y..core::cmp::min(y + rh, h) {
        for xx in x..core::cmp::min(x + rw, w - crate::ui::px(6)) {
            st.surf[yy * w + xx] = c;
        }
    }
}

/// Repaint `st.surf`: FILEVIEW's row painter, plus the selection band and the caret.
fn paint(st: &mut State) {
    let face = super::text::Face::Grid; // UIMETRICS: the dpi-sized mono grid face
    let (cw, ch) = (face.cell_w(), face.cell_h());
    let (w, h) = (st.w, st.h);
    for p in st.surf.iter_mut() {
        *p = theme::content_fill();
    }
    let sel = sel_range(st);
    let crow = row_of(&st.rows, st.caret);
    for r in 0..st.vis {
        let ri = st.top + r;
        let Some(&(a, b)) = st.rows.get(ri) else { break };
        let (a, b) = (a as usize, b as usize);
        let y = PAD() + r * ch;
        if let Some((s, e)) = sel {
            let lo = core::cmp::max(s, a);
            let hi = core::cmp::min(e, b + (st.text.get(b) == Some(&b'\n')) as usize);
            if lo < hi {
                fill(st, PAD() + (lo - a) * cw, y, (hi - lo) * cw, ch, theme::selection());
            }
        }
        super::text::draw_text(&mut st.surf, w, w - crate::ui::px(6), h, PAD(), y, &st.text[a..b], theme::content_text(), false, face);
        if ri == crow {
            fill(st, PAD() + (st.caret - a) * cw, y, crate::ui::px(2), ch, theme::content_text());
        }
    }
    let total = st.rows.len().max(1);
    let (x0, x1) = (w - crate::ui::px(5), w - crate::ui::px(1));
    let th = if total <= st.vis { h } else { (h * st.vis / total).max(crate::ui::px(8)) };
    let ty = if total <= st.vis { 0 } else { (h - th) * st.top / (total - st.vis) };
    for y in 0..h {
        let c = if y >= ty && y < ty + th { theme::scroll_thumb() } else { theme::scroll_track() };
        for x in x0..x1 {
            st.surf[y * w + x] = c;
        }
    }
}

fn sel_range(st: &State) -> Option<(usize, usize)> {
    st.anchor.filter(|&a| a != st.caret).map(|a| (core::cmp::min(a, st.caret), core::cmp::max(a, st.caret)))
}

fn del_sel(st: &mut State) -> bool {
    let Some((s, e)) = sel_range(st) else {
        st.anchor = None;
        return false;
    };
    st.text.drain(s..e);
    st.caret = s;
    st.anchor = None;
    true
}

fn insert(st: &mut State, bytes: &[u8]) -> bool {
    let b: Vec<u8> = bytes.iter().copied().filter(|&c| c == b'\n' || (0x20..=0x7e).contains(&c)).collect();
    if b.is_empty() {
        return false;
    }
    let removed = sel_range(st).map_or(0, |(s, e)| e - s);
    if st.text.len() - removed + b.len() > MAX_BYTES {
        serial_println!("[edit] refuse insert: over the {} byte cap", MAX_BYTES);
        return false;
    }
    del_sel(st);
    let at = st.caret;
    st.text.splice(at..at, b.iter().copied());
    st.caret += b.len();
    true
}

fn backspace(st: &mut State) -> bool {
    if del_sel(st) {
        return true;
    }
    if st.caret == 0 {
        return false;
    }
    st.caret -= 1;
    st.text.remove(st.caret);
    true
}

fn move_to(st: &mut State, pos: usize, extend: bool) {
    if extend {
        if st.anchor.is_none() {
            st.anchor = Some(st.caret);
        }
    } else {
        st.anchor = None;
    }
    st.caret = pos.min(st.text.len());
}

fn line_start(st: &State) -> usize {
    st.text[..st.caret].iter().rposition(|&b| b == b'\n').map_or(0, |p| p + 1)
}

fn line_end(st: &State) -> usize {
    st.text[st.caret..].iter().position(|&b| b == b'\n').map_or(st.text.len(), |p| st.caret + p)
}

fn vertical(st: &mut State, dir: isize, extend: bool) {
    let ri = row_of(&st.rows, st.caret);
    let want = st.want_col.unwrap_or(st.caret - st.rows[ri].0 as usize);
    let nr = ri as isize + dir;
    if nr < 0 || nr as usize >= st.rows.len() {
        return;
    }
    let (a, b) = st.rows[nr as usize];
    let pos = a as usize + want.min((b - a) as usize);
    move_to(st, pos, extend);
    st.want_col = Some(want);
}

/// One edit step on the state; `true` when the view needs repainting.
fn apply(st: &mut State, op: Op) -> bool {
    let mut keep_col = false;
    let mut edited = false;
    let changed = match op {
        Op::Type(c) => { edited = insert(st, &[c]); edited }
        Op::Paste => {
            let n = crate::video::clipboard::fresh_len();
            let mut buf = alloc::vec![0u8; n];
            let got = crate::video::clipboard::get(&mut buf);
            edited = insert(st, &buf[..got]);
            edited
        }
        Op::Back => { edited = backspace(st); edited }
        Op::Cut => match sel_range(st) {
            Some((s, e)) => {
                let _ = crate::video::clipboard::set(&st.text[s..e]);
                edited = del_sel(st);
                edited
            }
            None => false,
        },
        Op::Copy => {
            if let Some((s, e)) = sel_range(st) {
                let _ = crate::video::clipboard::set(&st.text[s..e]);
            }
            false
        }
        Op::SelectAll => { st.anchor = Some(0); st.caret = st.text.len(); true }
        Op::Left(x) => {
            let p = match (x, sel_range(st)) { (false, Some((s, _))) => s, _ => st.caret.saturating_sub(1) };
            move_to(st, p, x); true
        }
        Op::Right(x) => {
            let p = match (x, sel_range(st)) { (false, Some((_, e))) => e, _ => st.caret + 1 };
            move_to(st, p, x); true
        }
        Op::Home(x) => { let p = line_start(st); move_to(st, p, x); true }
        Op::End(x) => { let p = line_end(st); move_to(st, p, x); true }
        Op::Up => { keep_col = true; vertical(st, -1, false); true }
        Op::Down => { keep_col = true; vertical(st, 1, false); true }
        Op::Deselect => { let had = st.anchor.is_some(); st.anchor = None; had }
        Op::At(p) => { move_to(st, p, false); true }
    };
    if !keep_col {
        st.want_col = None;
    }
    if edited {
        st.edits += 1;
        st.dirty = true;
    }
    if changed {
        st.rows = layout_rows(&st.text, st.cols);
        let r = row_of(&st.rows, st.caret);
        if r < st.top {
            st.top = r;
        } else if r >= st.top + st.vis {
            st.top = r + 1 - st.vis;
        }
        st.top = core::cmp::min(st.top, st.rows.len().saturating_sub(st.vis));
        paint(st);
    }
    changed
}

#[derive(Clone, Copy)]
enum Op { Type(u8), Paste, Back, Cut, Copy, SelectAll, Left(bool), Right(bool), Home(bool), End(bool), Up, Down, Deselect, At(usize) }

/// Run `op` on the open buffer, then present and (when the dirty mark flipped) retitle.
fn run(op: Op) -> bool {
    let id = WIN.load(Ordering::Relaxed);
    let mut g = STATE.lock();
    let Some(st) = g.as_mut() else { return false };
    let changed = apply(st, op);
    let retitle = if st.dirty != st.title_dirty {
        st.title_dirty = st.dirty;
        crate::video::dialog::unsaved_declare(OWNER, leaf_of(&st.path).as_bytes(), st.dirty); // DIALOG2 (B404): the first real caller
        Some(title_of(&st.path, st.dirty))
    } else {
        None
    };
    drop(g);
    if let Some(t) = retitle {
        wm::retitle(id, t.as_bytes());
    }
    if changed {
        let _ = wm::present(id);
    }
    true
}

/// The Ctrl-S path. Returns `(bytes, lines, edits)` written. `print`: emit the witness line.
pub fn save(print: bool) -> Result<(usize, usize, usize), String> {
    use crate::fs::vfs::NodeKind;
    let id = WIN.load(Ordering::Relaxed);
    let (path, text, edits) = {
        let g = STATE.lock();
        let st = g.as_ref().ok_or_else(|| String::from("no editor open"))?;
        (st.path.clone(), st.text.clone(), st.edits)
    };
    let mt = crate::shell::vfs_mount_table();
    if matches!(mt.stat(&path), Ok(s) if matches!(s.kind, NodeKind::Dir)) {
        return Err(String::from("is a directory"));
    }
    let p = crate::fs::vfs::KERNEL_PRINCIPAL;
    let pref = mt.get_attr(&path, crate::fs::assoc::PREFERRED_KEY, p).ok(); // FILETYPE (B307): the save re-creates the file; a per-file opener choice survives it
    let _ = mt.unlink(&path, p);
    mt.create(&path, NodeKind::File, p).map_err(|e| alloc::format!("create: {:?}", e))?;
    if let Some(v) = pref { let _ = mt.set_attr(&path, crate::fs::assoc::PREFERRED_KEY, v, p); }
    let mut off = 0usize;
    while off < text.len() {
        let n = core::cmp::min(CHUNK, text.len() - off);
        let w = mt.write(&path, off as u64, &text[off..off + n], p).map_err(|e| alloc::format!("write: {:?}", e))?;
        if w == 0 {
            return Err(String::from("write: zero"));
        }
        off += w;
    }
    let _ = crate::fs::filetype::stamp_as_in(&mt, &path, crate::fs::filetype::saved_text_type(&path)); // FILETYPE (B307): a saved document is text/plain (QUARRY2: or the Markdown/JSON its name says)
    let lines = text.iter().filter(|&&b| b == b'\n').count() + (text.last().map_or(0, |&b| (b != b'\n') as usize));
    let retitle = {
        let mut g = STATE.lock();
        g.as_mut().map(|st| { st.dirty = false; st.title_dirty = false; title_of(&st.path, false) })
    };
    if let Some(t) = retitle {
        wm::retitle(id, t.as_bytes());
        let _ = wm::present(id);
    }
    crate::video::dialog::unsaved_declare(OWNER, b"", false); // DIALOG2: saved — clean
    serial_println!("[edit] saved path={} bytes={} lines={} edits={}", path, off, lines, edits);
    if print {
        serial_println!(":: TEXTEDIT: path={} bytes={} lines={} edits={} saved={} -> PASS ::", path, off, lines, edits, off);
    }
    Ok((off, lines, edits))
}

fn action_op(a: Action) -> Option<Op> {
    Some(match a {
        Action::Copy => Op::Copy,
        Action::Cut => Op::Cut,
        Action::Paste => Op::Paste,
        Action::SelectAll => Op::SelectAll,
        Action::SelectLeft => Op::Left(true),
        Action::SelectRight => Op::Right(true),
        Action::SelectLineStart => Op::Home(true),
        Action::SelectLineEnd => Op::End(true),
        Action::CursorLeft => Op::Left(false),
        Action::CursorRight => Op::Right(false),
        Action::CursorLineStart => Op::Home(false),
        Action::CursorLineEnd => Op::End(false),
        Action::Deselect => Op::Deselect,
        _ => return None,
    })
}

/// Keys, actions and wheel. `true` when consumed. Only while this window holds focus.
pub fn key_route(ev: crate::pal::Event) -> bool {
    if !is_open() || wm::focus_asid() != OWNER {
        return false;
    }
    match ev {
        crate::pal::Event::Wheel(d) => {
            let id = WIN.load(Ordering::Relaxed);
            let mut g = STATE.lock();
            let Some(st) = g.as_mut() else { return false };
            let t = (st.top as isize - d as isize * WHEEL_ROWS as isize).max(0) as usize;
            st.top = core::cmp::min(t, st.rows.len().saturating_sub(st.vis));
            paint(st);
            drop(g);
            let _ = wm::present(id);
            true
        }
        crate::pal::Event::Action(a) => match action_op(a) {
            Some(op) => run(op),
            None => false,
        },
        crate::pal::Event::Key(c) => match c {
            0x20..=0x7e => run(Op::Type(c)),
            0x0A | 0x0D => run(Op::Type(b'\n')),
            0x08 | 0x7F => run(Op::Back),
            0x1E => run(Op::Up),
            0x1F => run(Op::Down),
            // Left/Right bytes are typed alongside their resolved ACTION; the action moves the caret.
            0x1C | 0x1D => true,
            0x1B => run(Op::Deselect),
            0x13 => {
                if let Err(e) = save(true) {
                    serial_println!("[edit] save FAILED reason={}", e);
                }
                true
            }
            _ => false,
        },
        _ => false,
    }
}

/// Pointer: close box, caret placement, raise. `true` when consumed.
pub fn press_route(x: i32, y: i32) -> bool {
    let id = WIN.load(Ordering::Relaxed);
    if id == wm::WIN_NONE {
        return false;
    }
    match wm::hit_test(x, y) {
        Some((w, _, _)) if w == id => {}
        _ => return false,
    }
    if wm::close_box_hit(id, x, y) {
        serial_println!("[edit] press close win={} at ({},{})", id, x, y);
        if !ask_close() {
            close();
        }
        return true;
    }
    let Some(info) = wm::info(id) else { return false };
    if x < info.x as i32 || y < info.y as i32 {
        return false;
    }
    let sc = info.scale.max(1);
    let (lx, ly) = ((x as usize - info.x) / sc, (y as usize - info.y) / sc);
    if lx >= info.w || ly >= info.h {
        return false;
    }
    wm::focus_changed(OWNER);
    let face = super::text::Face::Grid; // UIMETRICS: the dpi-sized mono grid face
    let (cw, ch) = (face.cell_w(), face.cell_h());
    let pos = {
        let g = STATE.lock();
        let Some(st) = g.as_ref() else { return true };
        let ri = core::cmp::min(st.top + ly.saturating_sub(PAD()) / ch, st.rows.len() - 1);
        let (a, b) = st.rows[ri];
        a as usize + core::cmp::min((lx.saturating_sub(PAD()) + cw / 2) / cw.max(1), (b - a) as usize)
    };
    run(Op::At(pos));
    true
}

// ── The fixture ──────────────────────────────────────────────────────────────────

/// TEXTEDIT — open a scratch file under `/home`, type 40 characters through the shipped key path,
/// save, re-read through the mount table, compare, clean up. A boot with no writable `/home` falls
/// back to the root (`path=` says which).
#[cfg(feature = "witness")]
pub fn selftest() {
    use core::sync::atomic::AtomicBool;
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    let mut cands: Vec<String> = Vec::new();
    if let Some(p) = home_prefix() {
        cands.push(alloc::format!("{}TEXTEDIT.TMP", p));
    }
    cands.push(String::from("/home/TEXTEDIT.TMP"));
    cands.push(String::from("/TEXTEDIT.TMP"));
    let mut want: Vec<u8> = Vec::new();
    for i in 0..40u8 {
        want.push(b'a' + (i % 26));
    }
    let mut last_err = String::from("no candidate");
    for path in cands.iter() {
        if let Err(e) = open(path) {
            last_err = alloc::format!("open {}: {}", path, e);
            break;
        }
        for &c in want.iter() {
            apply_direct(Op::Type(c));
        }
        let res = save(false);
        let back = crate::shell::vfs_mount_table().read(path, 0, 4096).unwrap_or_default();
        let _ = crate::shell::vfs_mount_table().unlink(path, crate::fs::vfs::KERNEL_PRINCIPAL);
        close();
        match res {
            Ok((bytes, lines, edits)) => {
                let ok = back == want && bytes == 40 && edits == 40;
                serial_println!(":: TEXTEDIT: path={} bytes={} lines={} edits={} saved={} -> {} ::", path, bytes, lines, edits, back.len(), if ok { "PASS" } else { "FAIL" });
                return;
            }
            Err(e) => last_err = alloc::format!("save {}: {}", path, e),
        }
    }
    serial_println!(":: TEXTEDIT: refused reason={} -> FAIL ::", last_err);
}

#[cfg(feature = "witness")]
fn apply_direct(op: Op) {
    let _ = run(op);
}

/// FILEOPEN — the directory [`may_edit`] treats as the user's own (`/home/<user>/`, or `/home/` with no login).
#[cfg(feature = "witness")]
pub fn home_dir() -> String {
    home_prefix().unwrap_or_else(|| String::from("/home/"))
}

/// KERNELFONT2 (B363) M4: the faces loaded or were restyled — repaint the open window once (no window: nothing).
pub fn font_repaint() {
    let id = WIN.load(Ordering::Relaxed);
    if id == wm::WIN_NONE {
        return;
    }
    let mut g = STATE.lock();
    let Some(st) = g.as_mut() else { return };
    paint(st);
    drop(g);
    let _ = wm::present(id);
}

// ── DIALOG2 (rmbp-ledger B404) — a dirty close ASKS (the dialog's first real app caller) ──────────────────────────

fn leaf_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The close box on a DIRTY buffer: a SHEET on this window — Don't Save · Cancel · Save (Save the default, Esc =
/// Cancel). `false` when the buffer is clean (close at once) or the sheet could not be raised (closes as before).
fn ask_close() -> bool {
    let (dirty, name) = match STATE.lock().as_ref() {
        Some(st) => (st.dirty, String::from(leaf_of(&st.path))),
        None => return false,
    };
    if !dirty {
        return false;
    }
    let msg = alloc::format!("Do you want to save the changes you made to {}?", name);
    let mut d = crate::video::dialog::Dlg::new(crate::video::dialog::Icon::Caution, b"Text Editor", msg.as_bytes(), b"Your changes will be lost if you don't save them.", &[b"Don't Save", b"Cancel", b"Save"]);
    d.owner = OWNER;
    d.sheet_win = WIN.load(Ordering::Relaxed);
    d.user = true;
    d.act = crate::video::dialog::Act::EditClose;
    if !crate::video::dialog::post(d) {
        return false;
    }
    crate::video::dialog::open_now();
    serial_println!("[edit] close asks (dirty) -> sheet path={}", name);
    true
}

/// The close sheet's answer (`ix`: 0 Don't Save, 1 Cancel, 2 Save).
pub fn close_answer(ix: u8) {
    match ix {
        0 => close(),
        2 => match save(true) {
            Ok(_) => close(),
            Err(e) => serial_println!("[edit] save on close FAILED ({}) — the window stays", e),
        },
        _ => serial_println!("[edit] close cancelled — the window stays"),
    }
}
