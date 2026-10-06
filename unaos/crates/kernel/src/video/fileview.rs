// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! FILEVIEW — a read-only text viewer window: what a double-click on `NOTES.TXT` opens.
//!
//! Quarry launched ELFs and (with `facet`) showed PNGs; every other file said `no opener`. This is
//! the second opener, and it is deliberately the smallest one that is a real window: a monospace
//! grid painted into a cached-RAM surface with `font::draw_text` (the console's glyph path), scrolled
//! by arrows / wheel / space, titled with the FILE's name (R36), closed by the close box.
//!
//! * [`open`] reads through `crate::shell::vfs_mount_table()`, at most [`MAX_BYTES`]; a longer file
//!   shows its head and a final `...truncated` row. Long lines wrap at the window's column count.
//! * [`request_open`] is the click-router-safe door (a latch drained by [`service`], chained from
//!   `quarry::live::service`) — the same stack-depth reason `facet::request_open` documents.
//! * Keys and wheel arrive through [`key_route`], chained from `quarry::live::key_route` (the router
//!   files are byte-identity-critical); the press through [`press_route`], chained from
//!   `quarry::live::press_route`.
//!
//! Text is bytes: printable ASCII is drawn, `\t` is four spaces, `\r` is dropped, anything else is
//! `?` (the face carries no glyph for it). One viewer window at a time; opening another file
//! replaces the first.
//!
//! Witness: `:: FILEVIEW: path=<p> bytes=<n> lines=<n> rows=<n> wrapped=<n> -> PASS ::` ([`selftest`]).

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::video::{theme, wm};

/// Kernel-furniture owner slot (`+ 5`, after Facet's `+ 4`).
pub const OWNER: u64 = wm::KERNEL_OWNER_BASE + 5;
const _: () = assert!(OWNER != wm::KERNEL_OWNER_CONSOLE && OWNER != wm::KERNEL_OWNER_DESKTOP);
#[cfg(feature = "quarry")] const _: () = assert!(OWNER != super::quarry::live::OWNER);

/// Read ceiling, bytes.
pub const MAX_BYTES: usize = 256 * 1024;
const CHUNK: usize = 16 * 1024;
#[allow(non_snake_case)] #[inline] fn WIN_W() -> usize { crate::ui::px(720) } // UIMETRICS (B372): a NATIVE window — physical px at the panel's dpi scale, drawn at scale 1
#[allow(non_snake_case)] #[inline] fn WIN_H() -> usize { crate::ui::px(480) }
#[allow(non_snake_case)] #[inline] fn PAD() -> usize { crate::ui::px(6) }
const WHEEL_ROWS: usize = 3;
const TAIL: &str = "...truncated";

static WIN: AtomicU32 = AtomicU32::new(wm::WIN_NONE);
static PENDING: crate::sync::Mutex<Option<String>> = crate::sync::Mutex::new(None);
static STATE: crate::sync::Mutex<Option<State>> = crate::sync::Mutex::new(None);

struct State {
    path: String,
    /// Wrapped display rows, each a byte range into `text`.
    rows: Vec<(u32, u32)>,
    text: Vec<u8>,
    top: usize,
    vis: usize,
    cols: usize,
    w: usize,
    h: usize,
    surf: Vec<u32>,
    /// QUARRY2 (B336): styled ranges of `text` from a renderer (`richtext`); empty = plain text.
    spans: Vec<richtext::Span>,
}

/// What [`layout`] found.
pub struct Layout {
    pub rows: Vec<(u32, u32)>,
    pub lines: usize,
    pub wrapped: usize,
}

/// Sanitise raw bytes to drawable ASCII (see module header). Pure.
pub fn sanitize(raw: &[u8]) -> Vec<u8> {
    let mut o = Vec::with_capacity(raw.len());
    for &b in raw {
        match b {
            b'\n' => o.push(b'\n'),
            b'\r' => {}
            b'\t' => o.extend_from_slice(b"    "),
            0x20..=0x7e => o.push(b),
            _ => o.push(b'?'),
        }
    }
    o
}

/// Split `text` (already sanitised) into display rows of at most `cols` bytes. `lines` counts logical
/// lines (a trailing newline does not open one more); `wrapped` counts the EXTRA rows wrapping made.
/// Pure.
pub fn layout(text: &[u8], cols: usize) -> Layout {
    let cols = cols.max(1);
    let mut rows: Vec<(u32, u32)> = Vec::new();
    let (mut lines, mut wrapped) = (0usize, 0usize);
    let mut i = 0usize;
    while i < text.len() {
        let end = text[i..].iter().position(|&b| b == b'\n').map(|p| i + p).unwrap_or(text.len());
        lines += 1;
        let mut s = i;
        if s == end {
            rows.push((s as u32, s as u32));
        }
        while s < end {
            let e = core::cmp::min(s + cols, end);
            rows.push((s as u32, e as u32));
            if e < end {
                wrapped += 1;
            }
            s = e;
        }
        i = end + 1;
    }
    Layout { rows, lines, wrapped }
}

/// Clamp a scroll offset so the last page stays full. Pure.
pub fn clamp_top(top: usize, rows: usize, vis: usize) -> usize {
    core::cmp::min(top, rows.saturating_sub(vis))
}

fn title_of(path: &str) -> String {
    String::from(path.rsplit('/').next().unwrap_or(path))
}

/// Is this a TEXT name by the one extension table (`fs::filetype::EXT_TABLE`). FILETYPE (B307):
/// Quarry no longer routes through this — it opens by type — and a dotless name is no longer text by
/// fiat; this answers only for the viewer's own fixture, which picks a file to show. Pure.
pub fn is_text_name(name: &str) -> bool {
    crate::fs::filetype::by_extension(name) == Some(crate::fs::filetype::TEXT_PLAIN)
}

pub fn is_open() -> bool {
    WIN.load(Ordering::Relaxed) != wm::WIN_NONE
}

pub fn shown() -> String {
    STATE.lock().as_ref().map(|s| s.path.clone()).unwrap_or_default()
}

/// Latch a path for [`service`] (click-router safe).
pub fn request_open(path: &str) {
    *PENDING.lock() = Some(String::from(path));
}

/// QUARRY2 (B336): latch `path` to open with renderer `kind` (`"markdown"` / `"json"`).
pub fn request_open_styled(path: &str, kind: &str) {
    *PENDING_STYLED.lock() = Some((String::from(path), String::from(kind)));
}
static PENDING_STYLED: crate::sync::Mutex<Option<(String, String)>> = crate::sync::Mutex::new(None);

/// QUARRY2 (B336): open `path` rendered by `kind` — read, sanitise, render, then the one window body.
pub fn open_styled(path: &str, kind: &str) -> Result<(usize, usize, usize, usize), String> {
    let (raw, trunc) = read_capped(path)?;
    let clean = sanitize(&raw);
    let r = richtext::render(kind, &clean);
    let mut text: Vec<u8> = Vec::new();
    let mut spans = r.spans;
    if let Some(why) = r.note {
        let head = alloc::format!("({} shown as written: {})\n", kind, why);
        let n = head.len() as u32;
        for sp in spans.iter_mut() {
            sp.start += n;
            sp.end += n;
        }
        spans.insert(0, richtext::Span { start: 0, end: n - 1, tint: richtext::Tint::Dim, bold: false });
        text.extend_from_slice(head.as_bytes());
    }
    text.extend_from_slice(&r.text);
    serial_println!("[fileview] render path={} kind={} spans={} note={}", path, kind, spans.len(), r.note.unwrap_or("-"));
    open_inner(path, text, spans, raw.len(), trunc)
}

/// Drain the latch. Chained from `quarry::live::service`.
pub fn service() {
    let styled = PENDING_STYLED.lock().take();
    if let Some((p, k)) = styled {
        match open_styled(&p, &k) {
            Ok(_) => serial_println!("[quarry] open TEXT consumed=viewer render={} path={}", k, p),
            Err(e) => serial_println!("[fileview] refuse path={} render={} reason={}", p, k, e),
        }
    }
    let want = PENDING.lock().take();
    if let Some(p) = want {
        match open(&p) {
            Ok(_) => serial_println!("[quarry] open TEXT consumed=viewer path={}", p),
            Err(e) => {
                serial_println!("[quarry] open TEXT consumed=refused path={} reason={}", p, e);
                serial_println!("[fileview] refuse path={} reason={}", p, e);
            }
        }
    }
}

/// Read up to [`MAX_BYTES`] of `path`; `(bytes, truncated)`.
fn read_capped(path: &str) -> Result<(Vec<u8>, bool), String> {
    let mt = crate::shell::vfs_mount_table();
    let st = mt.stat(path).map_err(|e| alloc::format!("vfs: {:?}", e))?;
    if matches!(st.kind, crate::fs::vfs::NodeKind::Dir) {
        return Err(String::from("is a directory"));
    }
    let want = core::cmp::min(st.size as usize, MAX_BYTES);
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
    Ok((out, st.size as usize > MAX_BYTES))
}

/// **Open `path` in a viewer window.** Returns `(bytes, lines, rows, wrapped)` on success.
pub fn open(path: &str) -> Result<(usize, usize, usize, usize), String> {
    let (raw, trunc) = read_capped(path)?;
    open_bytes(path, &raw, trunc)
}

/// [`open`]'s body over bytes already in hand (also the fixture's fallback door).
pub fn open_bytes(path: &str, raw: &[u8], truncated: bool) -> Result<(usize, usize, usize, usize), String> {
    open_inner(path, sanitize(raw), Vec::new(), raw.len(), truncated)
}

/// The one window body over drawable text (and its styled ranges, QUARRY2).
fn open_inner(path: &str, text: Vec<u8>, spans: Vec<richtext::Span>, n_bytes: usize, truncated: bool) -> Result<(usize, usize, usize, usize), String> {
    let mut text = text;
    if truncated {
        if !text.is_empty() && *text.last().unwrap() != b'\n' {
            text.push(b'\n');
        }
        text.extend_from_slice(TAIL.as_bytes());
    }
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
    let lay = layout(&text, cols);
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
    let (rows_n, lines_n, wrapped_n) = (lay.rows.len(), lay.lines, lay.wrapped);
    let mut st = State {
        path: String::from(path),
        rows: lay.rows,
        text,
        top: 0,
        vis,
        cols,
        w,
        h,
        surf,
        spans,
    };
    paint(&mut st);
    let base = st.surf.as_ptr() as usize;
    let title = title_of(path);
    // The Vec<u32> lives in STATE for the window's life; `close` drops the row before the buffer.
    let id = wm::create_at_native(OWNER, base, len * 4, w as u32, h as u32, (w * 4) as u32, title.as_bytes(), ox + wm::BORDER(), oy + wm::TITLE_H() + wm::BORDER());
    if id == wm::WIN_NONE {
        return Err(String::from("window create failed"));
    }
    *STATE.lock() = Some(st);
    WIN.store(id, Ordering::Relaxed);
    wm::winid_register_holder(&WIN, "fileview");
    wm::focus_changed(OWNER);
    let _ = wm::present(id);
    serial_println!(
        "[fileview] open win={} path={} bytes={} lines={} rows={} wrapped={} cols={} vis={} truncated={}",
        id, path, n_bytes, lines_n, rows_n, wrapped_n, cols, vis, truncated as u8
    );
    Ok((n_bytes, lines_n, rows_n, wrapped_n))
}

/// Close the window; the surface is freed after the row stops naming it.
pub fn close() {
    let id = WIN.swap(wm::WIN_NONE, Ordering::Relaxed);
    if id == wm::WIN_NONE {
        return;
    }
    wm::close(id);
    *STATE.lock() = None;
    serial_println!("[fileview] closed win={}", id);
}

/// Repaint `st.surf` for `st.top`.
fn paint(st: &mut State) {
    let face = super::text::Face::Grid; // UIMETRICS: the dpi-sized mono grid face
    let (cw, ch) = (face.cell_w(), face.cell_h());
    let _ = cw;
    let (w, h) = (st.w, st.h);
    for p in st.surf.iter_mut() {
        *p = theme::content_fill();
    }
    for r in 0..st.vis {
        let Some(&(a, b)) = st.rows.get(st.top + r) else { break };
        if !st.spans.is_empty() {
            paint_styled_row(st, a as usize, b as usize, PAD() + r * ch); // QUARRY2 (B336)
            continue;
        }
        let s = &st.text[a as usize..b as usize];
        super::text::draw_text(&mut st.surf, w, w - crate::ui::px(6), h, PAD(), PAD() + r * ch, s, theme::content_text(), false, face);
    }
    // Scroll thumb on the right edge (proportional; full height when everything fits).
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

/// Scroll to `top` (clamped), repaint, present. `true` when the view moved.
fn scroll_to(top: isize) -> bool {
    let id = WIN.load(Ordering::Relaxed);
    let mut g = STATE.lock();
    let Some(st) = g.as_mut() else { return false };
    let t = clamp_top(top.max(0) as usize, st.rows.len(), st.vis);
    if t == st.top {
        return false;
    }
    st.top = t;
    paint(st);
    drop(g);
    let _ = wm::present(id);
    true
}

fn cur() -> Option<(isize, usize, usize)> {
    STATE.lock().as_ref().map(|s| (s.top as isize, s.vis, s.rows.len()))
}

/// Keys and wheel. `true` when consumed. Only while this window holds focus.
pub fn key_route(ev: crate::pal::Event) -> bool {
    if !is_open() || wm::focus_asid() != OWNER {
        return false;
    }
    let Some((top, vis, n)) = cur() else { return false };
    match ev {
        crate::pal::Event::Wheel(d) => {
            // Positive = wheel away = content moves down = view moves UP.
            scroll_to(top - d as isize * WHEEL_ROWS as isize);
            true
        }
        crate::pal::Event::Key(c) => {
            let page = vis.saturating_sub(1).max(1) as isize;
            match c {
                0x1F => scroll_to(top - 1),
                0x1E => scroll_to(top + 1),
                // PageUp/PageDown have no decoded byte on either HID path yet: space/`b` page.
                b' ' => scroll_to(top + page),
                b'b' | b'B' => scroll_to(top - page),
                b'g' => scroll_to(0),
                b'G' => scroll_to(n as isize),
                _ => return false,
            };
            true
        }
        _ => false,
    }
}

/// Pointer: close box, and raise on a press in the content. `true` when consumed.
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
        serial_println!("[fileview] press close win={} at ({},{})", id, x, y);
        close();
        return true;
    }
    let Some(info) = wm::info(id) else { return false };
    if x < info.x as i32 || y < info.y as i32 {
        return false;
    }
    let sc = info.scale.max(1);
    if (x as usize - info.x) / sc >= info.w || (y as usize - info.y) / sc >= info.h {
        return false;
    }
    wm::focus_changed(OWNER);
    true
}

// ── The fixture ─────────────────────────────────────────────────────────────────────────────────

/// FILEVIEW — open a file the boot image carries, count what the viewer laid out, close it.
///
/// The file is the first plain root entry with a text-ish name (`README`, `*.TXT`, `*.MD`…) found by
/// listing `/` through the mount table. A boot whose volume is not bound yet (or carries none) falls
/// back to an in-memory body and says so in `path=` (`mem:FILEVIEW.TXT`) — the layout/scroll legs are
/// the claim there, and the file leg is the claim wherever a volume exists. Legs: rows > 0, a scroll
/// step moves the view, and the window closes.
#[cfg(feature = "witness")]
pub fn selftest() {
    use core::sync::atomic::AtomicBool;
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    let mut chosen: Option<String> = None;
    {
        let mt = crate::shell::vfs_mount_table();
        if let Ok(ents) = mt.read_dir("/") {
            for e in ents.iter() {
                if !matches!(e.kind, crate::fs::vfs::NodeKind::Dir) && is_text_name(&e.name) && !e.name.starts_with('.') {
                    chosen = Some(alloc::format!("/{}", e.name));
                    break;
                }
            }
        }
    }
    let res = match chosen {
        Some(p) => open(&p).map(|r| (p, r)),
        None => {
            let mut body = String::new();
            for i in 0..80 {
                body.push_str(&alloc::format!("line {} of the FILEVIEW fixture body, long enough that the narrow columns wrap it onto a second row when the window is small {}\n", i, "x".repeat(60)));
            }
            let p = String::from("mem:FILEVIEW.TXT");
            open_bytes(&p, body.as_bytes(), false).map(|r| (p, r))
        }
    };
    match res {
        Ok((p, (bytes, lines, rows, wrapped))) => {
            let (top0, vis, _) = cur().unwrap_or((0, 0, 0));
            let moved = rows > vis && scroll_to(top0 + 1);
            let scroll_ok = rows <= vis || moved;
            let shown_ok = shown() == p && is_open();
            close();
            let closed = !is_open();
            let ok = rows > 0 && lines > 0 && scroll_ok && shown_ok && closed;
            serial_println!(
                ":: FILEVIEW: path={} bytes={} lines={} rows={} wrapped={} -> {} ::",
                p, bytes, lines, rows, wrapped, if ok { "PASS" } else { "FAIL" }
            );
        }
        Err(e) => serial_println!(":: FILEVIEW: refused reason={} -> FAIL ::", e),
    }
}

/// HELPVERB (R75) — open in-memory text (a `man` page) in the viewer under `title`. Same body as [`open`].
pub fn open_text(title: &str, text: &str) -> Result<(usize, usize, usize, usize), String> {
    open_bytes(title, text.as_bytes(), false)
}

// ── QUARRY2 (B336): styled rows ─────────────────────────────────────────────────────────────────

/// The colour of a renderer tint (`richtext` names what a range IS; the theme says how it looks).
fn tint_ink(t: richtext::Tint) -> u32 {
    use richtext::Tint;
    match t {
        Tint::Plain | Tint::Punct => theme::content_text(),
        Tint::Heading | Tint::Key => theme::accent(),
        Tint::Dim | Tint::Quote => theme::title_text_inactive(),
        Tint::Code | Tint::Str => crate::video::theme::syntax_code(),
        Tint::Num => crate::video::theme::syntax_num(),
        Tint::Lit => crate::video::theme::syntax_lit(),
    }
}

/// Draw display row `a..b` of `st.text` at `y`, cut at span edges (monospace: x = column).
fn paint_styled_row(st: &mut State, a: usize, b: usize, y: usize) {
    let face = super::text::Face::Grid; // UIMETRICS: the dpi-sized mono grid face
    let cw = face.cell_w();
    let (w, h) = (st.w, st.h);
    let mut p = a;
    while p < b {
        let k = st.spans.partition_point(|s| (s.end as usize) <= p);
        let (end, ink, bold) = match st.spans.get(k) {
            Some(s) if (s.start as usize) <= p => ((s.end as usize).min(b), tint_ink(s.tint), s.bold),
            Some(s) => ((s.start as usize).min(b), theme::content_text(), false),
            None => (b, theme::content_text(), false),
        };
        let end = end.max(p + 1);
        super::text::draw_text(&mut st.surf, w, w - crate::ui::px(6), h, PAD() + (p - a) * cw, y, &st.text[p..end], ink, bold, face);
        p = end;
    }
}

// QUARRY2 (B336): the Markdown and JSON renderers — a child module (no `video/mod.rs` line).
#[path = "richtext.rs"]
pub mod richtext;

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

/// QUARRY3 (rmbp-ledger B413): Quick Look's text body — THIS viewer's read, sanitise, renderer (`richtext`, for
/// `kind` = `markdown` / `json`; `""` = plain) and painter over a caller-sized `w x h` surface, no window. Quarry's
/// Quick Look blits it into its panel, so a preview is the viewer's own pixels, never a second text renderer.
/// Returns the surface and the number of wrapped rows the file made.
pub fn quicklook_body(path: &str, kind: &str, w: usize, h: usize) -> Result<(Vec<u32>, usize), String> {
    let (raw, _truncated) = read_capped(path)?;
    let clean = sanitize(&raw);
    let (text, spans) = if kind.is_empty() {
        (clean, Vec::new())
    } else {
        let r = richtext::render(kind, &clean);
        (r.text, r.spans)
    };
    let face = super::text::Face::Grid;
    let (cw, ch) = (face.cell_w(), face.cell_h());
    let cols = w.saturating_sub(2 * PAD() + crate::ui::px(6)) / cw.max(1);
    let vis = h.saturating_sub(2 * PAD()) / ch.max(1);
    if cols < 8 || vis < 1 {
        return Err(String::from("panel below floor"));
    }
    let lay = layout(&text, cols);
    let mut surf: Vec<u32> = Vec::new();
    if surf.try_reserve_exact(w * h).is_err() {
        return Err(String::from("out of memory"));
    }
    surf.resize(w * h, theme::content_fill());
    let rows_n = lay.rows.len();
    let mut st = State { path: String::from(path), rows: lay.rows, text, top: 0, vis, cols, w, h, surf, spans };
    paint(&mut st);
    Ok((st.surf, rows_n))
}
