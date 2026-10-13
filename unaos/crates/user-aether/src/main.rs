#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// AETHERMETAL M1 (LEDGER SR66): `APPS/AETHER.ELF` — Aether, UnaOS's browser (CODEX §2: Aether owns the Web), as ONE
// ring-3 program on the image, in LUMEN.ELF's shape (LUMENAPP B323, R82). Peter (2026-10-05): "all the more reason
// to get aether working".
//
// WHAT IT IS. A window over one HTML page read off the volume. The parser, the cascade, the LAYOUT and the text are
// the shared no_std cores (seam `shared-core`): html_core (WHATWG tokenizer + tree construction into an arena DOM),
// css_core (Syntax 3, Selectors 4, the cascade sort — matched over html_core's arena through `style::El`),
// layout_core (LAYOUTCORE SR76, AETHERMETAL M2: the host engine's box generation, taffy style mapping, inline
// formatter, tables and margin collapsing — the SAME engine handlers/aether lays out with; M1's in-crate
// formatter is gone), font_core::ui (the engine the kernel's video::text and Lumen draw with, the DejaVu faces off
// the volume).
//
// PIPELINE. read (/system/doc/welcome.html, SYS_PATH_READ, else SYS_OPEN) → html_core::parse_document →
// style::compute (<style> + style="", every winning declaration through layout_core::css::apply_declaration) →
// layout_core::layout (box tree from the tag defaults, the cascade folded on, taffy + the inline formatter at the
// window's width, text measured by txt::Fonts) → paint::paint (the LayoutTree into a back buffer) → the surface →
// SYS_WIN_PRESENT. A resize lays out again at the new width; a scroll repaints.
//
// KEYS. Up/Down scroll a line (20 px), the wheel three, PageUp/PageDown (and Space) a window less one line,
// Home/End (and Cmd/Ctrl+Home/End) the top and the bottom. Esc, or the window's close, exits.
//
// WIRE. `:: AETHER: start src=<path|open|none> bytes=<n> font=<dejavu-sans font_kib=<n>|font8x8 font_why=<w>> ::`
// once, then after the first paint ONE report the kernel's `tests aether` reads at the console seam (aether.rs):
// `:: AETHER: page <path> -> PASS|FAIL painted=<n> lines=<n> blocks=<n> elements=<n> rules=<n> doc_h=<n> ms=<n> ::`
// — `painted` the window's pixels that differ from the canvas colour, `lines` the line boxes of the page; PASS when
// the page was read and at least one line and one pixel were drawn (FAIL carries `reason=` first).
//
// AETHERMETAL M3 (LEDGER SR96): INTERACTION — the window answers the INPUT_EV ring's pointer and keys (R108's pane
// contract: a surface in, an input stream out, a resize). Model and wire in `ui.rs`; here, the dispatch:
//
// * POINTER. x86 hands ring 3 no window origin to map MOUSE_ABS through (UNAMETAL's note, user-una), so the pointer
//   is this program's own estimate, moved by MOUSE_REL deltas and clamped to the window, and DRAWN here (an arrow;
//   over a link a hand — the cursor change; a compositor cursor-shape verb does not exist, so the system cursor is
//   owed, not faked). Every move hit-tests with `layout_core::hit::hit` (the host's selection.rs hit test, one
//   function both rings call): over an `a[href]` the status line names the href and `:: AETHER: hover -> PASS
//   href=<h> ::` is written once per link entered.
// * CLICK (press + release, < 3 px between): over a link, NAVIGATE — the href resolved against the page's own
//   `file://` URL by `html_core::url` (WHATWG URL), a `file:` path read off the volume (its `/boot` twin too),
//   parsed, cascaded and laid out as the first page was; the old page's arena is freed. Session history is a list
//   of paths: Backspace / Left / mouse button 4 go Back, Right / button 5 Forward. Wire `:: AETHER: nav -> PASS
//   how=<click|enter|back|fwd> path=<p> lines=<n> hist=<i>/<n> ::` (FAIL carries `reason=`).
// * DRAG (left button): SELECT — the anchor is the hit's nearest grapheme boundary at the press, the focus follows
//   the pointer, the highlight is painted (`paint::Mark`); past the window's top or bottom the page scrolls. On the
//   release `:: AETHER: select -> PASS chars=<n> ::`. The middle button drags the page (grab scroll); the wheel
//   scrolls three lines. Cmd+A selects all, the deselect action clears.
// * COPY: Ctrl+C (KEY_DOWN 0x03) or Cmd+C (INPUT_EV_ACTION 3) → `SYS_CLIP_SET` (LUMENUX's verb, LUMEN.ELF's copy):
//   `:: AETHER: clip -> PASS set=<rc> chars=<n> ::` (FAIL with the refusal's code: -EACCES when not focused).
// * KEYS. Tab focuses the next link (outlined, scrolled into view; `:: AETHER: focus -> PASS href=<h>
//   at=<x0>,<y>,<x1> ::` — the link's first text box in window pixels, what `tests aether` aims the pointer at),
//   Enter follows it. Ctrl+F (0x06) or `/` opens the FIND BAR over the page text: typing searches (ASCII case
//   folded), Enter steps to the next match, Esc closes; matches paint yellow, the current one orange. Esc with
//   nothing open exits, as before.

extern crate alloc;

mod heap;
mod paint;
mod style;
mod sys;
mod txt;
mod ui;

use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use html_core::NodeId;
use layout_core::hit::TextBox;
use layout_core::{FontSel, NodeMap};
use una_abi::{
    input_ev_payload, input_ev_type, INPUT_EV_ACTION, INPUT_EV_BUTTON, INPUT_EV_CLOSE_REQ, INPUT_EV_KEY_DOWN, INPUT_EV_MOUSE_REL, INPUT_EV_WHEEL,
    INPUT_EV_WIN_RESIZE, KEY_DOWN, KEY_ESC, KEY_LEFT, KEY_RIGHT, KEY_UP, SYS_INPUT_POLL, SYS_WIN_CREATE, SYS_WIN_PRESENT,
};

use sys::{exit, now_ms, sleep_ms, sys, write};

#[global_allocator]
static HEAP: heap::Heap = heap::Heap::sbrk();

/// The compositor's window ceiling (FB_WIN_MAX_W/H on x86): the surface is always this stride.
pub const MAX_W: i32 = 288;
pub const MAX_H: i32 = 288;
const STRIDE: usize = MAX_W as usize * 4;
/// The page this M1 opens (staged by the builder), and its card twin under a UnaFS root.
const PAGES: [&[u8]; 2] = [b"/system/doc/welcome.html", b"/boot/system/doc/welcome.html"];
const PAGE_MAX: usize = 1024 * 1024;
/// One arrow-key step, px.
const STEP: f32 = 20.0;

const ACTION_LINE_START: u64 = 15;
const ACTION_LINE_END: u64 = 16;
const ACTION_PAGE_UP: u64 = 22;
const ACTION_PAGE_DOWN: u64 = 23;
const ACTION_TOP: u64 = 24;
const ACTION_BOTTOM: u64 = 25;
const ACTION_COPY: u64 = 3;
const ACTION_SELECT_ALL: u64 = 6;
const ACTION_DESELECT: u64 = 12;
/// A press and release closer than this (px, |dx| + |dy|) is a click; farther, a drag.
const DRAG_PX: i32 = 3;
/// How long a flash stays on the status line (ms).
const FLASH_MS: u64 = 2500;

/// The page the window shows when the file cannot be read (the reason is in it, and on the wire).
const MISSING: &str = "<!doctype html><title>Aether</title><style>body{color:#5a1010}</style>\
<h1>Aether</h1><p>The page <code>/system/doc/welcome.html</code> could not be read from the volume.</p>";

// ---- console lines --------------------------------------------------------------------------------
struct Line {
    b: [u8; 240],
    n: usize,
}
impl Line {
    fn new(s: &[u8]) -> Line {
        let mut l = Line { b: [0; 240], n: 0 };
        l.put(s);
        l
    }
    fn put(&mut self, s: &[u8]) -> &mut Self {
        for &c in s {
            if self.n < self.b.len() - 1 {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
        self
    }
    fn dec(&mut self, v: i64) -> &mut Self {
        if v < 0 {
            self.put(b"-");
        }
        let mut v = v.unsigned_abs();
        let mut d = [0u8; 20];
        let mut i = d.len();
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.put(&d[i..])
    }
    fn wire(&mut self) {
        self.b[self.n] = b'\n';
        write(&self.b[..self.n + 1]);
    }
}

// ---- the window -------------------------------------------------------------------------------------

/// The window landmarks: the RO info page at base + 0x4000, surface slot 0 at base + 0x5000 — an elf-model
/// program finds them at the FIXED slot base (RING3WIN), as LUMEN.ELF does.
#[cfg(target_arch = "x86_64")]
fn landmark_base() -> u64 {
    una_abi::USER_BASE_X86
}
#[cfg(not(target_arch = "x86_64"))]
fn landmark_base() -> u64 {
    match una_abi::args() {
        Some(a) if a.window_base() != 0 => a.window_base(),
        _ => {
            Line::new(b":: AETHER: no args page (window base unknown) ::").wire();
            exit(1)
        }
    }
}

struct Page {
    doc: &'static html_core::Document,
    sty: style::Styled,
    tree: Option<style::Tree>,
    doc_h: f32,
    lines: usize,
    blocks: usize,
    bg: u32,
    // ---- AETHERMETAL M3 (SR96) ----
    /// The path this page was read from (the base its relative hrefs resolve against).
    path: String,
    /// Inherited paint per box (the painter's and the hit test's font).
    inks: NodeMap<paint::Ink>,
    /// Every text fragment in document coordinates, paint order (`layout_core::hit::text_boxes`).
    boxes: Vec<TextBox>,
    flat: ui::Flat,
    /// Per text box, the `a[href]` it sits in.
    box_link: Vec<Option<NodeId>>,
    /// Every `a[href]`, document order (Tab).
    links: Vec<NodeId>,
}

impl Page {
    /// Parse `html` (read from `path`) into a page of its own arena, cascade it at the window's size.
    fn open(html: &str, path: String, w: i32, h: i32) -> Page {
        // The arena outlives every handle into it (the layout tree's `N`s): it is freed by `free`, after them.
        let doc: &'static html_core::Document = alloc::boxed::Box::leak(alloc::boxed::Box::new(html_core::parse_document(html, html_core::ParseOpts::default())));
        let sty = style::compute(doc, w as f32, h as f32);
        let links = ui::links(doc);
        Page {
            doc,
            sty,
            tree: None,
            doc_h: 0.0,
            lines: 0,
            blocks: 0,
            bg: 0x00FF_FFFF,
            path,
            inks: NodeMap::new(),
            boxes: Vec::new(),
            flat: ui::Flat::default(),
            box_link: Vec::new(),
            links,
        }
    }

    /// Drop the page and then its arena (nothing else holds a handle into it: links are `NodeId`s).
    fn free(self) {
        let doc = self.doc as *const html_core::Document as *mut html_core::Document;
        drop(self);
        // SAFETY: `doc` came from `Box::leak` in `open`; the only borrowers (the tree's node handles) were dropped
        // with `self` above.
        unsafe { drop(alloc::boxed::Box::from_raw(doc)) };
    }
}

struct App {
    win: i64,
    surf: *mut u8,
    flags: *const u32,
    w: i32,
    h: i32,
    scroll: f32,
    dirty: bool,
    relayout: bool,
    // ---- AETHERMETAL M3 (SR96) ----
    /// This program's pointer estimate (window px) and whether it has moved yet (drawn only then).
    ptr: (i32, i32),
    ptr_seen: bool,
    buttons: u8,
    /// A left press is down: the travel since it, and whether it became a drag.
    pressed: bool,
    travel: i32,
    dragged: bool,
    /// The selection's anchor and (anchor, focus), offsets into `Page::flat`.
    anchor: Option<usize>,
    sel: Option<(usize, usize)>,
    hover: Option<NodeId>,
    /// The Tab-focused link (index into `Page::links`).
    focus: Option<usize>,
    find: Option<ui::Find>,
    flash: Option<(String, u64)>,
    hist: Vec<String>,
    at: usize,
}

impl App {
    fn max_scroll(&self, p: &Page) -> f32 {
        (p.doc_h - self.h as f32).max(0.0)
    }
    fn scroll_to(&mut self, v: f32, p: &Page) {
        let v = v.clamp(0.0, self.max_scroll(p));
        if v != self.scroll {
            self.scroll = v;
            self.dirty = true;
        }
    }
    /// Bring the document band [`top`, `bottom`) into the window above the status line.
    fn reveal(&mut self, top: f32, bottom: f32, p: &Page) {
        let view = (self.h - paint::STATUS_H) as f32;
        if top < self.scroll {
            self.scroll_to(top - 4.0, p);
        } else if bottom > self.scroll + view {
            self.scroll_to(bottom - view + 4.0, p);
        }
    }
    fn flash(&mut self, s: String) {
        self.flash = Some((s, now_ms() + FLASH_MS));
        self.dirty = true;
    }
    /// Forget the per-page interaction (a new page, a relayout).
    fn reset_view(&mut self) {
        self.anchor = None;
        self.sel = None;
        self.hover = None;
        self.focus = None;
        self.pressed = false;
        self.dragged = false;
        self.dirty = true;
    }
    fn visible(&self) -> bool {
        (unsafe { self.flags.read_volatile() } & 2) == 0 // VUGMIN: bit 1 = every window hidden
    }
    /// Copy the back buffer onto the surface (stride MAX_W) and present.
    fn blit(&self, cv: &paint::Canvas) {
        for y in 0..cv.h.min(MAX_H) {
            let row = unsafe { self.surf.add(y as usize * STRIDE) as *mut u32 };
            for x in 0..cv.w.min(MAX_W) {
                let c = cv.px[(y * cv.w + x) as usize];
                unsafe { row.add(x as usize).write_volatile(0xFF00_0000 | c) };
            }
        }
        sys(SYS_WIN_PRESENT, self.win as u64, 0, 0, 0);
    }
}

/// Lay the page out at the window's size with layout_core (the host's engine): the line boxes of every inline
/// formatting context are `lines`, the block-level boxes `blocks`, the root box's height `doc_h`.
fn lay(p: &mut Page, fonts: &mut txt::Fonts, w: i32, h: i32) {
    if p.doc.document_element().is_none() {
        p.tree = None;
        p.doc_h = 0.0;
        return;
    }
    let tree = layout_core::layout(&style::Doc(p.doc), &p.sty, fonts, (w as f32, h as f32));
    p.lines = tree.inline.values().map(|il| il.lines.len()).sum();
    p.blocks = tree
        .node_map
        .iter()
        .filter(|(id, n)| layout_core::DomNode::is_element(*n) && layout_core::is_block_level(&tree, **id))
        .count();
    p.doc_h = tree.taffy.layout(tree.root_node).map(|l| l.size.height).unwrap_or(0.0);
    p.bg = paint::canvas_bg(&tree);
    // AETHERMETAL M3 (SR96): what the pointer and the keys act on.
    p.inks = paint::inks(&tree);
    p.boxes = layout_core::hit::text_boxes(&tree);
    p.flat = ui::Flat::build(&tree, &p.boxes);
    p.box_link = ui::link_of(p.doc, &tree, &p.boxes);
    p.tree = Some(tree);
}

// ---- AETHERMETAL M3 (SR96): the interaction ---------------------------------------------------------------

/// The shared hit test at window pixel `ptr`: document coordinates against the scroll, the caret measured in the
/// box's own face.
fn hit_at(p: &Page, fonts: &txt::Fonts, ptr: (i32, i32), scroll: f32) -> Option<layout_core::hit::Hit> {
    let tree = p.tree.as_ref()?;
    let (px, py) = (ptr.0 as f32, ptr.1 as f32 + scroll);
    layout_core::hit::hit(
        tree,
        &p.boxes,
        |_| (px, py),
        |i, text, b| {
            if b == 0 {
                return 0.0;
            }
            let (sel, size) = p.inks.get(&p.boxes[i].node).map(|k| (k.sel, k.size)).unwrap_or((FontSel::default(), 16.0));
            fonts.0.borrow_mut().measure(&text[..b.min(text.len())], sel, size)
        },
        |n: &style::N| n.id.0,
    )
}

/// The flat-text offset nearest the pointer.
fn flat_at(p: &Page, fonts: &txt::Fonts, ptr: (i32, i32), scroll: f32) -> Option<usize> {
    hit_at(p, fonts, ptr, scroll).map(|h| p.flat.at(h.index, h.near))
}

/// Re-hit the pointer for hover: the link under it (the hit must be INSIDE a box, not merely nearest).
fn hover_at(app: &mut App, p: &Page, fonts: &txt::Fonts) {
    let link = hit_at(p, fonts, app.ptr, app.scroll).filter(|h| h.inside).and_then(|h| p.box_link.get(h.index).copied().flatten());
    if link != app.hover {
        app.hover = link;
        app.dirty = true;
        if let Some(l) = link {
            let h = ui::href(p.doc, l);
            Line::new(b":: AETHER: hover -> PASS href=").put(cut(h.as_bytes(), 64)).put(b" ::").wire();
        }
    }
}

fn cut(b: &[u8], n: usize) -> &[u8] {
    &b[..b.len().min(n)]
}

/// An HTML-escaped copy of `s` (the error page quotes a path).
fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '&' => o.push_str("&amp;"),
            _ => o.push(c),
        }
    }
    o
}

/// Resolve `href` against the page at `cur` (`file://<cur>`, WHATWG URL): the volume path and the fragment.
fn resolve(cur: &str, href: &str) -> Result<(String, Option<String>), &'static [u8]> {
    let base = alloc::format!("file://{}", cur);
    let u = html_core::url::Url::parse_str_with_base(href, Some(&base)).map_err(|_| b"bad-url" as &[u8])?;
    if u.scheme() != "file" {
        return Err(b"not-a-file-url");
    }
    let path = html_core::url::percent::percent_decode_str(&u.pathname());
    Ok((path, u.fragment().map(String::from)))
}

/// Read `path` off the volume (its `/boot` twin when the root is the card): (bytes, the path that answered) or the
/// first refusal's code.
fn fetch(path: &str) -> Result<(Vec<u8>, String), i64> {
    let mut err = 0i64;
    let twin = alloc::format!("/boot{}", path);
    let tries: [&str; 2] = [path, if path.starts_with("/system/") { twin.as_str() } else { "" }];
    for t in tries.iter().filter(|t| !t.is_empty()) {
        match sys::read_file(t.as_bytes(), PAGE_MAX) {
            Ok((v, _)) if !v.is_empty() => return Ok((v, String::from(*t))),
            Ok(_) => {}
            Err(e) => {
                if err == 0 {
                    err = e;
                }
            }
        }
    }
    Err(err)
}

/// Load `path` as the page (`push` = a new history entry; Back/Forward move along the list instead).
fn load(app: &mut App, page: &mut Page, fonts: &mut txt::Fonts, path: &str, how: &[u8], push: bool) {
    let (html, got, code) = match fetch(path) {
        Ok((b, got)) => (String::from_utf8_lossy(&b).into_owned(), Some(got), 0),
        Err(e) => (
            alloc::format!(
                "<!doctype html><title>Aether</title><style>body{{color:#5a1010}}</style><h1>Not found</h1>\
<p>The page <code>{}</code> could not be read from the volume (code {}).</p><p>Backspace goes back.</p>",
                esc(path),
                e
            ),
            None,
            e,
        ),
    };
    let read = got.is_some();
    let new = Page::open(&html, got.unwrap_or_else(|| String::from(path)), app.w, app.h);
    drop(html);
    let old = core::mem::replace(page, new);
    old.free();
    lay(page, fonts, app.w, app.h);
    app.scroll = 0.0;
    app.find = None;
    app.reset_view();
    if push {
        app.hist.truncate(app.at + 1);
        app.hist.push(page.path.clone());
        app.at = app.hist.len() - 1;
    }
    let mut l = Line::new(b":: AETHER: nav -> ");
    if read && page.lines > 0 {
        l.put(b"PASS how=").put(how);
    } else {
        l.put(b"FAIL reason=").put(if read { b"no-lines" as &[u8] } else { b"no-page" }).put(b" code=").dec(code).put(b" how=").put(how);
    }
    l.put(b" path=").put(cut(page.path.as_bytes(), 72)).put(b" lines=").dec(page.lines as i64);
    l.put(b" hist=").dec(app.at as i64 + 1).put(b"/").dec(app.hist.len() as i64).put(b" ::");
    l.wire();
    app.flash(if read { page.path.clone() } else { alloc::format!("not found: {}", path) });
}

/// Follow `href` from the current page.
fn follow(app: &mut App, page: &mut Page, fonts: &mut txt::Fonts, href: &str, how: &[u8]) {
    match resolve(&page.path, href) {
        Err(r) => {
            Line::new(b":: AETHER: nav -> FAIL reason=").put(r).put(b" how=").put(how).put(b" href=").put(cut(href.as_bytes(), 64)).put(b" ::").wire();
            app.flash(alloc::format!("cannot open {}", href));
        }
        Ok((path, frag)) if path == page.path && frag.is_some() => {
            // Same document: scroll to the element with that id (its first text box).
            let id = frag.unwrap_or_default();
            let target = page.doc.descendants(html_core::Document::ROOT).find(|&n| page.doc.element(n).is_some_and(|e| e.attr("id") == Some(id.as_str())));
            let top = target.and_then(|t| {
                page.boxes.iter().position(|b| {
                    page.tree.as_ref().and_then(|tr| tr.node_map.get(&b.node)).is_some_and(|n| {
                        let mut c = Some(n.id);
                        while let Some(x) = c {
                            if x == t {
                                return true;
                            }
                            c = page.doc.parent(x);
                        }
                        false
                    })
                })
            });
            if let Some(i) = top {
                let b = page.boxes[i];
                app.scroll_to(b.top - 4.0, page);
            }
            let mut l = Line::new(b":: AETHER: nav -> ");
            l.put(if top.is_some() { b"PASS" as &[u8] } else { b"FAIL reason=no-anchor" }).put(b" how=").put(how);
            l.put(b" path=").put(cut(page.path.as_bytes(), 72)).put(b" frag=").put(cut(id.as_bytes(), 32)).put(b" ::");
            l.wire();
        }
        Ok((path, _)) => load(app, page, fonts, &path, how, true),
    }
}

fn back(app: &mut App, page: &mut Page, fonts: &mut txt::Fonts) {
    if app.at > 0 {
        app.at -= 1;
        let p = app.hist[app.at].clone();
        load(app, page, fonts, &p, b"back", false);
    }
}

fn forward(app: &mut App, page: &mut Page, fonts: &mut txt::Fonts) {
    if app.at + 1 < app.hist.len() {
        app.at += 1;
        let p = app.hist[app.at].clone();
        load(app, page, fonts, &p, b"fwd", false);
    }
}

/// Tab: focus the next link, scroll it into view, say where its first text box is.
fn focus_next(app: &mut App, page: &Page) {
    if page.links.is_empty() {
        Line::new(b":: AETHER: focus -> FAIL reason=no-links ::").wire();
        return;
    }
    let k = app.focus.map_or(0, |k| (k + 1) % page.links.len());
    app.focus = Some(k);
    app.dirty = true;
    let link = page.links[k];
    let h = ui::href(page.doc, link);
    match page.box_link.iter().position(|l| *l == Some(link)) {
        Some(i) => {
            let b = page.boxes[i];
            app.reveal(b.top, b.bottom, page);
            let y = ((b.top + b.bottom) / 2.0 - app.scroll) as i64;
            let mut l = Line::new(b":: AETHER: focus -> PASS href=");
            l.put(cut(h.as_bytes(), 48)).put(b" at=").dec(b.x as i64 + 1).put(b",").dec(y).put(b",").dec((b.x + b.w) as i64).put(b" ::");
            l.wire();
        }
        None => {
            Line::new(b":: AETHER: focus -> FAIL reason=not-painted href=").put(cut(h.as_bytes(), 48)).put(b" ::").wire();
        }
    }
    app.flash(h);
}

/// Ctrl+C / Cmd+C: the selection to the compositor's clipboard (SYS_CLIP_SET).
fn copy(app: &mut App, page: &Page) {
    let Some((a, b)) = app.sel.filter(|(a, b)| a != b) else {
        Line::new(b":: AETHER: clip -> FAIL reason=no-selection ::").wire();
        app.flash(String::from("nothing selected"));
        return;
    };
    let text = page.flat.slice(a, b);
    let chars = text.chars().count();
    let t = ui::clip_text(text);
    let r = sys(una_abi::SYS_CLIP_SET, t.as_ptr() as u64, t.len() as u64, 0, 0);
    let mut l = Line::new(b":: AETHER: clip -> ");
    l.put(if r >= 0 { b"PASS" as &[u8] } else { b"FAIL reason=refused" });
    l.put(b" set=").dec(r).put(b" chars=").dec(chars as i64).put(b" ::");
    l.wire();
    app.flash(if r >= 0 { alloc::format!("copied {} chars", chars) } else { alloc::format!("copy refused ({})", r) });
}

/// Re-run the find over the page text; `step` moves to the next match.
fn find_refresh(app: &mut App, page: &Page, step: bool) {
    let Some(f) = app.find.as_mut() else { return };
    if step {
        if !f.hits.is_empty() {
            f.cur = (f.cur + 1) % f.hits.len();
        }
    } else {
        f.hits = page.flat.find(&f.q);
        let scroll = app.scroll;
        f.cur = f.hits.iter().position(|&h| page.flat.box_at(h).is_some_and(|i| page.boxes[i].top >= scroll)).unwrap_or(0);
    }
    let at = f.hits.get(f.cur).copied();
    let (n, cur) = (f.hits.len(), f.cur);
    if step {
        let mut l = Line::new(b":: AETHER: find -> ");
        l.put(if n > 0 { b"PASS" as &[u8] } else { b"FAIL reason=no-match" }).put(b" hits=").dec(n as i64).put(b" cur=").dec(cur as i64 + 1).put(b" ::");
        l.wire();
    }
    if let Some(i) = at.and_then(|o| page.flat.box_at(o)) {
        let b = page.boxes[i];
        app.reveal(b.top, b.bottom, page);
    }
    app.dirty = true;
}

/// The highlights for the painter: find matches, then the selection on top.
fn marks(app: &App, page: &Page) -> NodeMap<Vec<paint::Mark>> {
    let mut m = NodeMap::new();
    if let Some(f) = &app.find {
        for (k, &h) in f.hits.iter().enumerate() {
            let kind = if k == f.cur { paint::MarkKind::FindCur } else { paint::MarkKind::Find };
            page.flat.marks(&page.boxes, h, h + f.q.len(), kind, &mut m);
        }
    }
    if let Some((a, b)) = app.sel {
        page.flat.marks(&page.boxes, a, b, paint::MarkKind::Select, &mut m);
    }
    m
}

/// Over the page: the focused link's outline, the status line, the pointer.
fn overlay(app: &App, page: &Page, cv: &mut paint::Canvas, fonts: &txt::Fonts) {
    if let Some(link) = app.focus.and_then(|k| page.links.get(k).copied()) {
        for (i, b) in page.boxes.iter().enumerate() {
            if page.box_link[i] == Some(link) {
                let y = (b.top - app.scroll) as i32;
                paint::outline(cv, b.x as i32 - 1, y, b.w as i32 + 2, (b.bottom - b.top) as i32, 0x002F_8FD8);
            }
        }
    }
    if let Some(f) = &app.find {
        let s = if f.hits.is_empty() {
            alloc::format!("Find: {}_", f.q)
        } else {
            alloc::format!("Find: {}_  {}/{}", f.q, f.cur + 1, f.hits.len())
        };
        paint::status(cv, fonts, &s, !f.q.is_empty() && f.hits.is_empty());
    } else if let Some(l) = app.hover {
        paint::status(cv, fonts, &ui::href(page.doc, l), false);
    } else if let Some((s, _)) = &app.flash {
        paint::status(cv, fonts, s, false);
    }
    if app.ptr_seen {
        paint::pointer(cv, app.ptr.0, app.ptr.1, app.hover.is_some());
    }
}

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    let _ = &APP_NOTE;
    let base = landmark_base();
    let t0 = now_ms();
    // The window first (a launch during the desktop's own start may meet a busy compositor: retry for 5 s).
    let mut win = sys(SYS_WIN_CREATE, MAX_W as u64, MAX_H as u64, 0, 0);
    let mut tries = 0;
    while win < 0 && tries < 50 {
        sleep_ms(100);
        win = sys(SYS_WIN_CREATE, MAX_W as u64, MAX_H as u64, 0, 0);
        tries += 1;
    }
    if win < 0 {
        let mut l = Line::new(b":: AETHER: SYS_WIN_CREATE refused code=");
        l.dec(win).put(b" ::");
        l.wire();
        exit(1);
    }
    let mut app = App {
        win,
        surf: (base + 0x5000) as *mut u8,
        flags: unsafe { ((base + 0x4000) as *const u32).add(0x20 / 4) },
        w: MAX_W,
        h: MAX_H,
        scroll: 0.0,
        dirty: true,
        relayout: false,
        ptr: (MAX_W / 2, MAX_H / 2),
        ptr_seen: false,
        buttons: 0,
        pressed: false,
        travel: 0,
        dragged: false,
        anchor: None,
        sel: None,
        hover: None,
        focus: None,
        find: None,
        flash: None,
        hist: Vec::new(),
        at: 0,
    };

    // The page off the volume.
    let mut src: Option<sys::Src> = None;
    let mut path: &[u8] = PAGES[0];
    let mut read_err = 0i64;
    let mut bytes: Vec<u8> = Vec::new();
    for p in PAGES {
        match sys::read_file(p, PAGE_MAX) {
            Ok((v, s)) if !v.is_empty() => {
                bytes = v;
                src = Some(s);
                path = p;
                break;
            }
            Ok(_) => read_err = 0,
            Err(e) => {
                if read_err == 0 {
                    read_err = e;
                }
            }
        }
    }
    // The faces.
    let (txt, why) = txt::load();

    let mut l = Line::new(b":: AETHER: start src=");
    match src {
        Some(s) => l.put(s.as_str()),
        None => l.put(b"none code=").dec(read_err),
    };
    l.put(b" bytes=").dec(bytes.len() as i64);
    match &why {
        None => {
            l.put(b" font=").put(txt.name()).put(b" font_kib=").dec((txt.bytes / 1024) as i64).put(b" faces=").dec(txt.faces as i64);
        }
        Some(w) => {
            l.put(b" font=font8x8 font_why=").put(w.name()).put(b" font_code=").dec(w.code());
        }
    }
    l.put(b" ::");
    l.wire();

    // Parse, cascade, box tree, layout.
    let t_parse = now_ms();
    let html = match src {
        Some(_) => alloc::string::String::from_utf8_lossy(&bytes).into_owned(),
        None => alloc::string::String::from(MISSING),
    };
    drop(bytes);
    // The page owns its arena (`Page::open`); a navigation frees it after the layout tree that borrows it.
    let mut page = Page::open(&html, String::from_utf8_lossy(path).into_owned(), app.w, app.h);
    drop(html);
    let mut fonts = txt::Fonts(RefCell::new(txt));
    lay(&mut page, &mut fonts, app.w, app.h);
    app.hist.push(page.path.clone());
    let mut cv = paint::Canvas::new(app.w, app.h);
    let mut reported = false;

    loop {
        let mut k = 0;
        while k < 32 {
            let ev = sys(SYS_INPUT_POLL, 0, 0, 0, 0);
            if ev < 0 {
                break;
            }
            let ev = ev as u64;
            let p = input_ev_payload(ev);
            let page_step = (app.h as f32 - STEP).max(STEP);
            match input_ev_type(ev) {
                // The find bar takes the typing while it is open (arrows still scroll).
                INPUT_EV_KEY_DOWN if app.find.is_some() && !matches!(p as u8, KEY_UP | KEY_DOWN) => match p as u8 {
                    KEY_ESC => {
                        app.find = None;
                        app.dirty = true;
                    }
                    b'\n' | b'\r' => find_refresh(&mut app, &page, true),
                    0x08 | 0x7F => {
                        if let Some(f) = app.find.as_mut() {
                            f.q.pop();
                        }
                        find_refresh(&mut app, &page, false);
                    }
                    c @ 0x20..=0x7E => {
                        if let Some(f) = app.find.as_mut() {
                            if f.q.len() < 48 {
                                f.q.push(c as char);
                            }
                        }
                        find_refresh(&mut app, &page, false);
                    }
                    _ => {}
                },
                INPUT_EV_KEY_DOWN => match p as u8 {
                    KEY_ESC if app.sel.is_some() || app.focus.is_some() => {
                        app.sel = None;
                        app.focus = None;
                        app.dirty = true;
                    }
                    KEY_ESC => exit(0),
                    KEY_UP => app.scroll_to(app.scroll - STEP, &page),
                    KEY_DOWN => app.scroll_to(app.scroll + STEP, &page),
                    b' ' => app.scroll_to(app.scroll + page_step, &page),
                    // M3: links, history, find, copy.
                    b'\t' => focus_next(&mut app, &page),
                    b'\n' | b'\r' => {
                        if let Some(l) = app.focus.and_then(|k| page.links.get(k).copied()) {
                            let h = ui::href(page.doc, l);
                            follow(&mut app, &mut page, &mut fonts, &h, b"enter");
                        }
                    }
                    0x08 | 0x7F | KEY_LEFT => back(&mut app, &mut page, &mut fonts),
                    KEY_RIGHT => forward(&mut app, &mut page, &mut fonts),
                    0x06 | b'/' => {
                        app.find = Some(ui::Find::default());
                        app.dirty = true;
                    }
                    0x03 => copy(&mut app, &page),
                    _ => {}
                },
                INPUT_EV_ACTION if matches!(p, ACTION_COPY | ACTION_SELECT_ALL | ACTION_DESELECT) => match p {
                    ACTION_COPY => copy(&mut app, &page),
                    ACTION_SELECT_ALL => {
                        app.sel = Some((0, page.flat.text.len()));
                        app.dirty = true;
                    }
                    _ => {
                        app.sel = None;
                        app.dirty = true;
                    }
                },
                INPUT_EV_MOUSE_REL => {
                    let (dx, dy) = ((p >> 16) as u16 as i16 as i32, p as u16 as i16 as i32);
                    let (nx, ny) = (app.ptr.0 + dx, app.ptr.1 + dy);
                    if app.buttons & 4 != 0 {
                        app.scroll_to(app.scroll - dy as f32, &page); // the middle button drags the page
                    } else if app.pressed {
                        // A selection dragged past the window's edge scrolls the page under it.
                        if ny < 0 {
                            app.scroll_to(app.scroll + ny as f32, &page);
                        } else if ny > app.h - 1 {
                            app.scroll_to(app.scroll + (ny - app.h + 1) as f32, &page);
                        }
                    }
                    app.ptr = (nx.clamp(0, app.w - 1), ny.clamp(0, app.h - 1));
                    app.ptr_seen = true;
                    app.dirty = true;
                    if app.pressed {
                        app.travel += dx.abs() + dy.abs();
                        if app.travel >= DRAG_PX {
                            app.dragged = true;
                        }
                    }
                    if app.pressed && app.dragged {
                        if let (Some(a), Some(f)) = (app.anchor, flat_at(&page, &fonts, app.ptr, app.scroll)) {
                            app.sel = Some((a, f));
                        }
                    } else {
                        hover_at(&mut app, &page, &fonts);
                    }
                }
                INPUT_EV_BUTTON => {
                    let (m, was) = (p as u8, app.buttons);
                    app.buttons = m;
                    if m & 1 != 0 && was & 1 == 0 {
                        app.pressed = true;
                        app.travel = 0;
                        app.dragged = false;
                        app.anchor = flat_at(&page, &fonts, app.ptr, app.scroll);
                        app.sel = None;
                        app.dirty = true;
                    }
                    if m & 1 == 0 && was & 1 != 0 && app.pressed {
                        app.pressed = false;
                        if app.dragged {
                            if let Some((a, b)) = app.sel.filter(|(a, b)| a != b) {
                                let n = page.flat.slice(a, b).chars().count();
                                Line::new(b":: AETHER: select -> PASS chars=").dec(n as i64).put(b" ::").wire();
                                app.flash(alloc::format!("{} chars selected", n));
                            }
                            hover_at(&mut app, &page, &fonts);
                        } else {
                            hover_at(&mut app, &page, &fonts);
                            if let Some(l) = app.hover {
                                let h = ui::href(page.doc, l);
                                follow(&mut app, &mut page, &mut fonts, &h, b"click");
                            }
                        }
                    }
                    if m & 8 != 0 && was & 8 == 0 {
                        back(&mut app, &mut page, &mut fonts);
                    }
                    if m & 16 != 0 && was & 16 == 0 {
                        forward(&mut app, &mut page, &mut fonts);
                    }
                }
                INPUT_EV_ACTION => match p {
                    ACTION_PAGE_UP => app.scroll_to(app.scroll - page_step, &page),
                    ACTION_PAGE_DOWN => app.scroll_to(app.scroll + page_step, &page),
                    ACTION_TOP | ACTION_LINE_START => app.scroll_to(0.0, &page),
                    ACTION_BOTTOM | ACTION_LINE_END => app.scroll_to(f32::MAX, &page),
                    _ => {}
                },
                INPUT_EV_WHEEL => {
                    let n = p as u8 as i8 as f32; // positive = toward the top (Lumen's sense)
                    app.scroll_to(app.scroll - n * 3.0 * STEP, &page);
                }
                INPUT_EV_WIN_RESIZE => {
                    let w = (((p >> 16) & 0xffff) as i32).clamp(64, MAX_W);
                    let h = ((p & 0xffff) as i32).clamp(64, MAX_H);
                    if w != app.w || h != app.h {
                        app.relayout = w != app.w;
                        app.w = w;
                        app.h = h;
                        app.dirty = true;
                    }
                }
                INPUT_EV_CLOSE_REQ => exit(0), // the WM asks us to quit: nothing unsaved
                _ => {}
            }
            k += 1;
        }
        if app.relayout {
            lay(&mut page, &mut fonts, app.w, app.h);
            app.relayout = false;
            app.scroll = app.scroll.clamp(0.0, app.max_scroll(&page));
            app.reset_view(); // the text boxes moved: selection offsets and the hover are stale
            find_refresh(&mut app, &page, false);
        }
        if app.flash.as_ref().is_some_and(|(_, t)| now_ms() >= *t) {
            app.flash = None;
            app.dirty = true;
        }
        if app.dirty && (app.visible() || !reported) {
            if cv.w != app.w || cv.h != app.h {
                cv = paint::Canvas::new(app.w, app.h);
            }
            match page.tree.as_ref() {
                Some(t) => paint::paint(&mut cv, t, app.scroll, page.bg, &fonts, &page.inks, &marks(&app, &page)),
                None => cv.fill(0, 0, cv.w, cv.h, page.bg),
            }
            if !reported {
                reported = true;
                report(path, src.is_some(), &page, &cv, now_ms().saturating_sub(t_parse), now_ms().saturating_sub(t0));
            }
            overlay(&app, &page, &mut cv, &fonts);
            app.blit(&cv);
            app.dirty = false;
        }
        sleep_ms(16);
    }
}

/// The one line `tests aether` reads (aether.rs): the verdict, then the numbers after it.
fn report(path: &[u8], read: bool, p: &Page, cv: &paint::Canvas, ms: u64, total_ms: u64) {
    let painted = cv.count_not(p.bg);
    let pass = read && p.lines > 0 && painted > 0;
    let mut l = Line::new(b":: AETHER: page ");
    l.put(path).put(if pass { b" -> PASS" as &[u8] } else { b" -> FAIL" });
    if !pass {
        l.put(if !read { b" reason=no-page" as &[u8] } else if p.lines == 0 { b" reason=no-lines" } else { b" reason=nothing-painted" });
    }
    l.put(b" painted=").dec(painted as i64);
    l.put(b" lines=").dec(p.lines as i64);
    l.put(b" blocks=").dec(p.blocks as i64);
    l.put(b" elements=").dec(p.sty.elements as i64);
    l.put(b" rules=").dec(p.sty.rules as i64);
    l.put(b" doc_h=").dec(p.doc_h as i64);
    l.put(b" ms=").dec(ms as i64);
    l.put(b" start_ms=").dec(total_ms as i64);
    l.put(b" heap_kib=").dec((HEAP.usage().1 / 1024) as i64);
    l.put(b" ::");
    l.wire();
    let _ = p.doc;
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    write(b":: AETHER: panic ::\n");
    exit(3)
}

/// EXECNAME (B322, R82): the launch declaration — it opens a window (SYS_WIN_CREATE), so a bare `aether` detaches
/// like `bg`. Kept by the x86 link script under a PT_NOTE header; read by `midden_core::app_note_flags`.
#[used]
#[link_section = ".note.unaos.app"]
static APP_NOTE: una_abi::AppNote = una_abi::AppNote::new(una_abi::APP_FLAG_WINDOWED);
