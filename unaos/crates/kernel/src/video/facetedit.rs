// CHARTER: Facet — shared-core face (Images; FACETEDIT, rmbp-ledger B900)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! FACETEDIT — Facet's kernel face: the image viewer's EDIT mode over the three shared cores (raster_core SR230,
//! brush_core SR231, adjust_core SR232), the on-UnaOS twin of `vessels/facet-view --edit` (RASTERFACE SR234).
//!
//! The door is the viewer's own key: `e` while the viewer holds the keyboard. The viewer's shown picture (its base
//! image, at most 2048x1536) becomes layer 0 of ONE raster_core `Document`, the viewer's row closes, and this module
//! mints ONE native window under Facet's own owner. This file is the WINDOW: the queue the router-depth hooks fill,
//! the service pass that drains it, the press -> hit-table -> gesture mapping, the canvas drag sampled on the pass
//! (the viewer's `drag_poll` shape), Save / Open through the VFS, and `tests facetedit`. The model and its gesture
//! -> op table are [`super::facetedit_tools`]; the paint and the hit table are [`super::facetedit_layers`]. No
//! editing logic lives here or in `facet.rs`: every edit is a core call through `History::commit`.
//!
//! Wire: one `:: FACET: gesture=<g> … -> ok|refused ::` per gesture (the host face's token);
//! `[facetedit] open path= doc=<w>x<h> win= surf=<w>x<h> layer_bytes=`; one close line (`close`, banner-cert's row).
//! View state lives in memory (owed FACETEDIT-PREFS: Principia's `system.facet.*`). Design and the knob-off
//! reasoning: `docs/dev/evidence/raster/FACETEDIT.md`.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::draw::Rect;
use super::facetedit_layers::{self as layers, Hit, Layout};
use super::facetedit_tools::{word, Editor, ADJUSTS, FIELDS};
use super::wm;
use crate::fs::vfs::{NodeKind, VfsError, KERNEL_PRINCIPAL as P};

/// Facet's owner: the editor replaces the viewer's row, so the one owner names one Facet window at a time.
pub const OWNER: u64 = super::facet::OWNER;

/// The largest container Open reads (raster_core's own caps bound the decode; this bounds the read).
const MAX_FILE: u64 = 64 * 1024 * 1024;
/// Bytes per VFS read / write (the viewer's `CHUNK`: FAT re-walks the chain per call).
const CHUNK: usize = 256 * 1024;
/// The container's extension (the host faces' `.unarastr`).
const EXT: &str = "unarastr";

static WIN: AtomicU32 = AtomicU32::new(wm::WIN_NONE);

/// The live editor: the model, its window surface (never reallocated while the row names it), the regions and
/// the hit table the last paint recorded.
struct State {
    ed: Editor,
    surf: Vec<u32>,
    lay: Layout,
    hits: Vec<(Rect, Hit)>,
}

static STATE: crate::sync::Mutex<Option<State>> = crate::sync::Mutex::new(None);

/// A gesture queued at router depth (surface coordinates), applied by [`service`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cmd {
    Press(i32, i32),
    Wheel(i8, i32, i32),
    Key(u8),
    Undo,
    Redo,
}

static CMDS: crate::sync::Mutex<Vec<Cmd>> = crate::sync::Mutex::new(Vec::new());
static CMDS_POSTED: super::svclatch::Latch = super::svclatch::Latch::new();
static OPEN_REQ: crate::sync::Mutex<Option<()>> = crate::sync::Mutex::new(None);
static OPEN_POSTED: super::svclatch::Latch = super::svclatch::Latch::new();
/// A canvas drag is live: [`service`] samples the pointer until the button comes up.
static DRAG_ON: AtomicBool = AtomicBool::new(false);

fn push(c: Cmd) {
    let mut q = CMDS.lock();
    if q.len() < 32 {
        q.push(c);
    }
    drop(q);
    CMDS_POSTED.post();
}

/// The viewer's `e` (router depth): only a latch; [`service`] opens.
pub fn request_from_viewer() {
    *OPEN_REQ.lock() = Some(());
    OPEN_POSTED.post();
}

/// Is the editor's window live?
pub fn is_open() -> bool {
    WIN.load(Ordering::Relaxed) != wm::WIN_NONE
}

/// Drain the open latch, the queue and a live drag. Chained from `facet::service` (a quiet pass: atomic loads).
pub fn service() {
    if OPEN_POSTED.take(&OPEN_REQ).is_some() {
        open_from_viewer();
    }
    for c in CMDS_POSTED.take_vec(&CMDS) {
        let mut g = STATE.lock();
        if let Some(st) = g.as_mut() {
            if apply(st, c, true) {
                present(st);
            }
        }
    }
    if DRAG_ON.load(Ordering::Relaxed) {
        drag_poll();
    }
}

/// `0xFFRRGGBB` base pixels -> straight RGBA bytes (alpha 255: the viewer dropped alpha).
fn to_rgba(px: &[u32]) -> Vec<u8> {
    let mut v = Vec::with_capacity(px.len() * 4);
    for p in px {
        v.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]);
    }
    v
}

/// `<folder>/<stem>.unarastr` beside the viewer's file.
fn save_path_of(path: &str) -> String {
    let cut = path.rfind('/').map_or(0, |i| i + 1);
    let leaf = &path[cut..];
    let stem = leaf.rfind('.').map_or(leaf, |i| &leaf[..i]);
    format!("{}{}.{}", &path[..cut], stem, EXT) // a sibling file's extension, not a dotfile
}

/// The viewer's picture becomes the document (on the service pass).
fn open_from_viewer() {
    let Some((path, px, bw, bh)) = super::facet::edit_handoff() else {
        serial_println!("[facetedit] open refused why=no-image (the viewer shows no picture)");
        return;
    };
    let rgba = to_rgba(&px);
    drop(px);
    match Editor::doc_of(bw as u32, bh as u32, rgba) {
        Ok(doc) => {
            let save = save_path_of(&path);
            open_doc(doc, &path, save);
        }
        Err(why) => serial_println!("{}", super::facetedit_tools::token("open", &format!("path={}", word(&path)), Err(why), 0, 0, 0, 0)),
    }
}

/// The window's device size: the logical window at the panel's scale, clamped to the work area.
fn window_size() -> Option<(usize, usize)> {
    let pi = crate::video::panel_info_nonblocking()?;
    let (pw, ph) = (pi.width, pi.height);
    let chrome_v = crate::ui_status::top_chrome_h(pw, ph) + crate::ui_status::chrome_h(ph) + wm::TITLE_H() + 2 * wm::BORDER();
    let w = super::metrics::size(layers::WIN_W).min(pw.saturating_sub(2 * wm::BORDER()));
    let h = super::metrics::size(layers::WIN_H).min(ph.saturating_sub(chrome_v));
    (w >= 320 && h >= 240).then_some((w, h))
}

/// Build the editor over `doc` (`path` is the picture it came from, `save` the container path) and, when the panel
/// is up, mint its window. Returns the window id (or `WIN_NONE`: the model only, for a headless fixture).
fn open_doc(doc: raster_core::Document, path: &str, save: String) -> wm::WinId {
    close();
    let (sw, sh) = window_size().unwrap_or((super::metrics::size(layers::WIN_W), super::metrics::size(layers::WIN_H)));
    let lay = Layout::of(sw, sh);
    let (dw, dh) = (doc.width, doc.height);
    let mut ed = Editor::with(doc, (lay.canvas.w as u32, lay.canvas.h as u32), save);
    ed.emit("open", &format!("path={} width={} height={}", word(path), dw, dh), Ok(()));
    let mut surf: Vec<u32> = Vec::new();
    if surf.try_reserve_exact(sw * sh).is_err() {
        serial_println!("[facetedit] open refused why=alloc({} bytes)", sw * sh * 4);
        return wm::WIN_NONE;
    }
    surf.resize(sw * sh, 0);
    let hits = layers::paint(&mut ed, &mut surf, &lay);
    let st = State { ed, surf, lay, hits };
    let id = mint(&st, path).unwrap_or(wm::WIN_NONE);
    serial_println!(
        "[facetedit] open path={} doc={}x{} win={} surf={}x{} layer_bytes={}",
        path, dw, dh, id, sw, sh, dw as usize * dh as usize * 4
    );
    if id != wm::WIN_NONE {
        let _ = wm::present(id);
    }
    *STATE.lock() = Some(st);
    id
}

/// Mint the native window over the state's surface, centred in the work area.
fn mint(st: &State, path: &str) -> Option<wm::WinId> {
    let pi = crate::video::panel_info_nonblocking()?;
    let (pw, ph) = (pi.width, pi.height);
    let (sw, sh) = (st.lay.w, st.lay.h);
    let (_s, ow, oh) = wm::spawn_geometry_native(sw, sh)?;
    let wtop = crate::ui_status::top_chrome_h(pw, ph);
    let ox = pw.saturating_sub(ow) / 2;
    let oy = wtop + ph.saturating_sub(wtop).saturating_sub(crate::ui_status::chrome_h(ph)).saturating_sub(oh) / 2;
    let leaf = path.rsplit('/').next().unwrap_or(path);
    let title = format!("{} - edit", leaf);
    let base = st.surf.as_ptr() as usize;
    let id = wm::create_at_native(OWNER, base, sw * sh * 4, sw as u32, sh as u32, (sw * 4) as u32, title.as_bytes(), ox + wm::BORDER(), oy + wm::TITLE_H() + wm::BORDER());
    if id == wm::WIN_NONE {
        return None;
    }
    WIN.store(id, Ordering::Relaxed);
    wm::winid_register_holder(&WIN, "facetedit");
    wm::focus_changed(OWNER);
    Some(id)
}

/// Repaint (the hit table with it) and present.
fn present(st: &mut State) {
    st.hits = layers::paint(&mut st.ed, &mut st.surf, &st.lay);
    let id = WIN.load(Ordering::Relaxed);
    if id != wm::WIN_NONE {
        let _ = wm::present(id);
    }
}

/// Close the window and drop the document (nothing is saved implicitly).
pub fn close() {
    let id = WIN.swap(wm::WIN_NONE, Ordering::Relaxed);
    DRAG_ON.store(false, Ordering::Relaxed);
    // `wm::close` FIRST, with no lock of ours held: the row stops naming the surface before it is freed.
    if id != wm::WIN_NONE {
        wm::close(id);
    }
    let tokens = STATE.lock().take().map(|s| s.ed.tokens).unwrap_or(0);
    if id != wm::WIN_NONE || tokens > 0 {
        serial_println!("[facetedit] closed win={} tokens={}", id, tokens);
    }
}

/// Apply one gesture to the state: the hit table decides what a press means, the model runs the ONE op. `live`
/// arms the pointer-sampled drag (a real press); the fixture drives the drag itself. True when the frame changed.
fn apply(st: &mut State, c: Cmd, live: bool) -> bool {
    let canvas = st.lay.canvas;
    match c {
        Cmd::Press(x, y) => match layers::hit_at(&st.hits, x, y) {
            Some((r, h)) => match h {
                Hit::Rail => st.ed.set_tool(layers::rail_tool(r, x)),
                Hit::Verb(v) => {
                    st.ed.verb(v);
                }
                Hit::File("save") => save(&mut st.ed),
                Hit::File(_) => open_saved(&mut st.ed),
                Hit::Adjust(i) => {
                    st.ed.adjust(ADJUSTS[i].0);
                }
                Hit::Row(i) => st.ed.select_layer(i),
                Hit::Eye(i) => st.ed.toggle_eye(i),
                Hit::Field(i) => {
                    let (key, _, max) = FIELDS[i];
                    st.ed.set_field(key, layers::slider_value(r, x, max));
                }
            },
            None if x >= canvas.x && y >= canvas.y && x < canvas.right() && y < canvas.bottom() => {
                st.ed.press(x - canvas.x, y - canvas.y);
                if live {
                    DRAG_ON.store(true, Ordering::Relaxed);
                }
                return false;
            }
            None => return false,
        },
        Cmd::Wheel(d, x, y) => {
            if d == 0 || x < canvas.x || y < canvas.y || x >= canvas.right() || y >= canvas.bottom() {
                return false;
            }
            st.ed.zoom(if d > 0 { 1 } else { -1 }, x - canvas.x, y - canvas.y);
        }
        Cmd::Key(k) => match k {
            b'+' | b'=' => st.ed.zoom_centre(1),
            b'-' => st.ed.zoom_centre(-1),
            b'Z' => st.ed.redo(),
            b' ' => {
                st.ed.pan_armed = true;
                return false;
            }
            _ => {
                // Escape: a drag in flight is dropped (no op).
                DRAG_ON.store(false, Ordering::Relaxed);
                st.ed.pan_armed = false;
                return st.ed.drag.take().is_some();
            }
        },
        Cmd::Undo => st.ed.undo(),
        Cmd::Redo => st.ed.redo(),
    }
    true
}

/// The pointer in this window's surface coordinates (a native row: scale 1).
fn pointer() -> Option<(i32, i32)> {
    let id = WIN.load(Ordering::Relaxed);
    let (pi, info) = (crate::video::panel_info_nonblocking()?, wm::info(id)?);
    let (x, y) = crate::pal::cursor::pos(pi.width as i32, pi.height as i32);
    Some((x - info.x as i32, y - info.y as i32))
}

/// The canvas drag: one sample per pass while the button is held; the release lands the tool's ONE op.
fn drag_poll() {
    let Some((sx, sy)) = pointer() else {
        DRAG_ON.store(false, Ordering::Relaxed);
        return;
    };
    let mut g = STATE.lock();
    let Some(st) = g.as_mut() else {
        DRAG_ON.store(false, Ordering::Relaxed);
        return;
    };
    let (rx, ry) = (sx - st.lay.canvas.x, sy - st.lay.canvas.y);
    if !crate::pal::cursor::button_down() {
        DRAG_ON.store(false, Ordering::Relaxed);
        st.ed.release(rx, ry);
        present(st);
        return;
    }
    if st.ed.motion(rx, ry) && st.ed.drag.as_ref().is_some_and(|d| d.pan) {
        present(st);
    }
}

/// Keys, Cmd-Z / Shift-Cmd-Z and the wheel, chained first from `facet::key_route` (router depth: QUEUES only).
pub fn key_route(ev: crate::pal::Event) -> bool {
    use crate::video::keymap::Action;
    let id = WIN.load(Ordering::Relaxed);
    if id == wm::WIN_NONE {
        return false;
    }
    match ev {
        crate::pal::Event::Wheel(d) => {
            let Some(pi) = crate::video::panel_info_nonblocking() else { return false };
            let (x, y) = crate::pal::cursor::pos(pi.width as i32, pi.height as i32);
            match (wm::hit_test(x, y), wm::info(id)) {
                (Some((w, _, _)), Some(info)) if w == id && d != 0 => {
                    push(Cmd::Wheel(d, x - info.x as i32, y - info.y as i32));
                    true
                }
                _ => false,
            }
        }
        crate::pal::Event::Action(a) if wm::focus_asid() == OWNER => match a {
            Action::Undo => {
                push(Cmd::Undo);
                true
            }
            Action::Redo => {
                push(Cmd::Redo);
                true
            }
            _ => false,
        },
        crate::pal::Event::Key(c) if wm::focus_asid() == OWNER => match c {
            b'+' | b'=' | b'-' | b'Z' | b' ' | 0x1B => {
                push(Cmd::Key(c));
                true
            }
            _ => false,
        },
        _ => false,
    }
}

/// A press, chained first from `facet::press_route` (router depth): the close disc closes; a press on the content
/// focuses and is QUEUED in surface coordinates for [`service`].
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
        close();
        return true;
    }
    let Some(info) = wm::info(id) else { return false };
    if x < info.x as i32 || y < info.y as i32 {
        return false; // chrome: the router owns it
    }
    let (sx, sy) = (x - info.x as i32, y - info.y as i32);
    if sx as usize >= info.w || sy as usize >= info.h {
        return false;
    }
    wm::focus_changed(OWNER);
    push(Cmd::Press(sx, sy));
    true
}

// ── Save / Open: raster_core's container through the VFS ─────────────────────────────────────────────────────────

/// The VFS's refusal as one token word.
fn vfs_word(e: VfsError) -> &'static str {
    match e {
        VfsError::NoSuchVolume => "vfs-no-such-volume",
        VfsError::NoSuchPath => "vfs-enoent",
        VfsError::NotADirectory => "vfs-enotdir",
        VfsError::IsADirectory => "vfs-eisdir",
        VfsError::Denied => "vfs-eacces",
        VfsError::Unsupported => "vfs-enotsup",
        VfsError::Backend(_) => "vfs-backend",
    }
}

fn write_file(path: &str, bytes: &[u8]) -> Result<(), &'static str> {
    let mt = crate::shell::vfs_mount_table();
    let _ = mt.unlink(path, P);
    mt.create(path, NodeKind::File, P).map_err(vfs_word)?;
    let mut off = 0usize;
    while off < bytes.len() {
        let end = (off + CHUNK).min(bytes.len());
        let n = mt.write(path, off as u64, &bytes[off..end], P).map_err(vfs_word)?;
        if n == 0 {
            return Err("vfs-short-write");
        }
        off += n;
    }
    Ok(())
}

fn read_file(path: &str) -> Result<Vec<u8>, &'static str> {
    let mt = crate::shell::vfs_mount_table();
    let st = mt.stat(path).map_err(vfs_word)?;
    if st.size == 0 || st.size > MAX_FILE {
        return Err("too-large");
    }
    let mut out: Vec<u8> = Vec::new();
    if out.try_reserve_exact(st.size as usize).is_err() {
        return Err("alloc");
    }
    while (out.len() as u64) < st.size {
        let want = (st.size - out.len() as u64).min(CHUNK as u64) as usize;
        let b = mt.read(path, out.len() as u64, want).map_err(vfs_word)?;
        if b.is_empty() {
            return Err("vfs-short-read");
        }
        out.extend_from_slice(&b);
    }
    Ok(out)
}

/// Save: `raster_core::encode` -> the container path (ops.rs `Op::Save`).
fn save(ed: &mut Editor) {
    let path = ed.save_path.clone();
    let r = raster_core::encode(&ed.doc).map_err(|e| e.word()).and_then(|b| write_file(&path, &b));
    ed.emit("save", &format!("path={}", word(&path)), r);
}

/// Open: the container path -> `raster_core::decode`; the history cleared (ops.rs `Op::Open`).
fn open_saved(ed: &mut Editor) {
    let path = ed.save_path.clone();
    let r = read_file(&path).and_then(|b| raster_core::decode(&b).map_err(|e| e.word())).map(|doc| ed.replace(doc));
    ed.emit("open", &format!("path={}", word(&path)), r);
}

// ── `tests facetedit` ────────────────────────────────────────────────────────────────────────────────────────────

/// The surface point of canvas cell corner `(cx, cy)` (+ `half` of a cell) under the editor's view.
#[cfg(feature = "witness")]
fn at_cell(st: &State, cx: i64, cy: i64, half: bool) -> (i32, i32) {
    let v = st.ed.view;
    let q = |c: i64| c * 256 + if half { 128 } else { 0 };
    let sc = |c: i64| if v.zoom >= 0 { q(c) << v.zoom } else { q(c) >> -v.zoom };
    let (rx, ry) = ((sc(cx) + v.ox) >> 8, (sc(cy) + v.oy) >> 8);
    (st.lay.canvas.x + rx as i32, st.lay.canvas.y + ry as i32)
}

/// The centre of the recorded control `h` (the fixture presses what the paint drew).
#[cfg(feature = "witness")]
fn centre_of(st: &State, h: Hit) -> (i32, i32) {
    st.hits.iter().find(|(_, k)| *k == h).map_or((-1, -1), |(r, _)| (r.x + r.w / 2, r.y + r.h / 2))
}

/// The surface point of rail segment `i`.
#[cfg(feature = "witness")]
fn rail_at(st: &State, i: usize) -> (i32, i32) {
    st.hits.iter().find(|(_, k)| *k == Hit::Rail).map_or((-1, -1), |(r, _)| (r.x + r.w * (2 * i as i32 + 1) / 16, r.y + r.h / 2))
}

/// A click on the canvas at a surface point: the press through the hit table, the release through the model (the
/// release `drag_poll` would make; the fixture holds no pointer).
#[cfg(feature = "witness")]
fn click(st: &mut State, p: (i32, i32)) {
    apply(st, Cmd::Press(p.0, p.1), false);
    st.ed.release(p.0 - st.lay.canvas.x, p.1 - st.lay.canvas.y);
}

/// FACETEDIT — the eight host-face KAT gestures on a generated 64x48 image, every verdict a readback.
#[cfg(feature = "witness")]
pub fn selftest() {
    let (w, h) = (64u32, 48u32);
    let mut rgba: Vec<u8> = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            rgba.extend_from_slice(&[(x * 4) as u8, (y * 5) as u8, ((x ^ y) * 4) as u8, 255]);
        }
    }
    let orig = rgba.clone();
    let home = crate::fs::trash::home_base();
    let save_p = format!("{}/FEDTEST.unarastr", home);
    let doc = match Editor::doc_of(w, h, rgba) {
        Ok(d) => d,
        Err(why) => {
            serial_println!(":: FACETEDIT: doc_of refused why={} -> FAIL ::", why);
            return;
        }
    };
    let id = open_doc(doc, &format!("{}/FEDTEST.PNG", home), save_p.clone());
    let yn = |b: bool| if b { "ok" } else { "bad" };
    let mut g = STATE.lock();
    let Some(st) = g.as_mut() else {
        serial_println!(":: FACETEDIT: zoom=bad pan=bad brush=bad layers=bad fill=bad invert=bad roundtrip=bad refused=bad tokens=0 -> FAIL ::");
        return;
    };
    let c = st.lay.canvas;
    let mid = (c.x + c.w / 2, c.y + c.h / 2);
    // 1. zoom: a wheel notch at the region centre doubles the scale, the cell under the centre stays.
    let (z0, cell0) = (st.ed.view.zoom, st.ed.view.cell(c.w / 2, c.h / 2));
    apply(st, Cmd::Wheel(1, mid.0, mid.1), false);
    let zoom_ok = st.ed.view.zoom == z0 + 1 && st.ed.view.cell(c.w / 2, c.h / 2) == cell0;
    present_model(st);
    // 2. pan: the Move segment, then a drag of (+7, +5): the origin moves by exactly that.
    let p = rail_at(st, 7);
    apply(st, Cmd::Press(p.0, p.1), false);
    let (ox, oy) = (st.ed.view.ox, st.ed.view.oy);
    apply(st, Cmd::Press(mid.0, mid.1), false);
    let (rx, ry) = (mid.0 - c.x, mid.1 - c.y);
    st.ed.motion(rx + 3, ry + 2);
    st.ed.release(rx + 7, ry + 5);
    let pan_ok = st.ed.tool == super::facetedit_tools::Tool::Move && st.ed.view.ox == ox + 7 * 256 && st.ed.view.oy == oy + 5 * 256;
    present_model(st);
    // 3. brush: colour (255, 0, 0, 255) through the field sliders, Brush, a click over cell (32, 24).
    for (i, full) in [(3usize, true), (4, false), (5, false), (6, true)] {
        let r = st.hits.iter().find(|(_, k)| *k == Hit::Field(i)).map(|(r, _)| *r).unwrap_or_default();
        let x = if full { r.right() - 1 } else { r.x };
        apply(st, Cmd::Press(x, r.y + r.h / 2), false);
    }
    let red = [255u8, 0, 0, 255];
    let p = rail_at(st, 4);
    apply(st, Cmd::Press(p.0, p.1), false);
    let p = at_cell(st, 32, 24, true);
    click(st, p);
    let brush_ok = st.ed.colour == red && st.ed.doc.layers.get(st.ed.active).and_then(|l| l.pixel(32, 24)) == Some(red);
    present_model(st);
    // 4. layers: the strip's `+` -> 2 layers; Cmd-Z -> 1.
    let p = centre_of(st, Hit::Verb("layer-add"));
    apply(st, Cmd::Press(p.0, p.1), false);
    let two = st.ed.doc.layers.len() == 2;
    present_model(st);
    apply(st, Cmd::Undo, false);
    let layers_ok = two && st.ed.doc.layers.len() == 1;
    present_model(st);
    // 5. fill: a layer, a Rect drag over cells (8, 8)..(24, 20), the Fill tool clicked.
    let p = centre_of(st, Hit::Verb("layer-add"));
    apply(st, Cmd::Press(p.0, p.1), false);
    present_model(st);
    let p = rail_at(st, 0);
    apply(st, Cmd::Press(p.0, p.1), false);
    let (a, b) = (at_cell(st, 8, 8, false), at_cell(st, 24, 20, false));
    apply(st, Cmd::Press(a.0, a.1), false);
    st.ed.release(b.0 - c.x, b.1 - c.y);
    let p = rail_at(st, 6);
    apply(st, Cmd::Press(p.0, p.1), false);
    click(st, mid);
    let px_at = |ed: &mut Editor, x: usize, y: usize| -> [u8; 4] {
        let comp = &ed.composite().0;
        let i = (y * w as usize + x) * 4;
        [comp[i], comp[i + 1], comp[i + 2], comp[i + 3]]
    };
    let o = |x: usize, y: usize| -> [u8; 4] {
        let i = (y * w as usize + x) * 4;
        [orig[i], orig[i + 1], orig[i + 2], orig[i + 3]]
    };
    let fill_ok = px_at(&mut st.ed, 10, 10) == red && px_at(&mut st.ed, 23, 19) == red && px_at(&mut st.ed, 50, 40) == o(50, 40) && px_at(&mut st.ed, 4, 4) == o(4, 4);
    present_model(st);
    // 6. invert twice through the adjust row: different after one, the original after two.
    let before = st.ed.composite().0.clone();
    let p = centre_of(st, Hit::Adjust(3));
    apply(st, Cmd::Press(p.0, p.1), false);
    let once = st.ed.composite().0 != before;
    apply(st, Cmd::Press(p.0, p.1), false);
    let invert_ok = once && st.ed.composite().0 == before;
    present_model(st);
    // 7. roundtrip: Save, then Open: the layer count and the composite's fnv agree.
    let (n0, f0) = (st.ed.doc.layers.len(), super::facetedit_tools::fnv1a(&st.ed.composite().0));
    let p = centre_of(st, Hit::File("save"));
    apply(st, Cmd::Press(p.0, p.1), false);
    let saved = st.ed.last_ok;
    let p = centre_of(st, Hit::File("open"));
    apply(st, Cmd::Press(p.0, p.1), false);
    let roundtrip_ok = saved && st.ed.last_ok && st.ed.doc.layers.len() == n0 && super::facetedit_tools::fnv1a(&st.ed.composite().0) == f0;
    present_model(st);
    // 8. refused: active := 5 unchecked, a Fill click: `why=no-layer`, the document and the history unchanged.
    let (f1, u1) = (super::facetedit_tools::fnv1a(&st.ed.composite().0), st.ed.hist.past.len());
    st.ed.active = 5;
    click(st, mid);
    let refused_ok = !st.ed.last_ok && st.ed.last_why == "no-layer" && super::facetedit_tools::fnv1a(&st.ed.composite().0) == f1 && st.ed.hist.past.len() == u1;
    let tokens = st.ed.tokens;
    let zoom = st.ed.view.scale_word();
    drop(g);
    if id != wm::WIN_NONE {
        let _ = wm::present(id);
    }
    close();
    let _ = crate::shell::vfs_mount_table().unlink(&save_p, P);
    let ok = zoom_ok && pan_ok && brush_ok && layers_ok && fill_ok && invert_ok && roundtrip_ok && refused_ok;
    serial_println!("[facetedit] fixture win={} doc={}x{} zoom={} path={}", if id == wm::WIN_NONE { String::from("none") } else { format!("{}", id) }, w, h, zoom, save_p);
    serial_println!(
        ":: FACETEDIT: zoom={} pan={} brush={} layers={} fill={} invert={} roundtrip={} refused={} tokens={} -> {} ::",
        yn(zoom_ok), yn(pan_ok), yn(brush_ok), yn(layers_ok), yn(fill_ok), yn(invert_ok), yn(roundtrip_ok), yn(refused_ok), tokens,
        if ok { "PASS" } else { "FAIL" }
    );
}

/// The fixture's repaint: the hit table follows the model (no present under the lock; one present at the end).
#[cfg(feature = "witness")]
fn present_model(st: &mut State) {
    st.hits = layers::paint(&mut st.ed, &mut st.surf, &st.lay);
}
