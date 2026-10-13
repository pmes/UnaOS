// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// CHARTER: Aether — shared-core (the host chrome's std-free MODEL linked by path; edit_core for the address field)
//
// AETHERMETAL4 (LEDGER SR109): THE BROWSER CHROME ON THE IMAGE — what the host face has (AETHERCHROME SR84), as
// ring-3 code over the SAME model. R104: the faces only PAINT the chrome; the model is `handlers/aether/src/chrome/`,
// whose seven model modules are std-free by design (its mod.rs: "the same model links into a ring-3 browser on the
// image"), so this file LINKS them by `#[path]` — the bytes the host compiles, never a copy — and adds only what a
// face adds: the strip and the toolbar painted in theme colours, the pointer and the keys, and the I/O (`store.rs`,
// the host's `host.rs` twin over SYS_PATH_*). R108's pane contract holds: the chrome is pixels on the one surface
// and verbs in from the one input ring; nothing widget-shaped.
//
// LAYOUT (window px). Row 1 (y 0..TAB_H) the TAB STRIP: one tab per `tabs::TabStrip` entry, the active one in the
// page's band with its close box, `+` at the end. Row 2 (y TAB_H..CHROME_H) the TOOLBAR: Back, Forward, the address
// field (the OMNIBOX), the bookmark star, zoom out, zoom in. The page sits below at `CHROME_H`, laid out at the
// LOGICAL size `(pw, ph - CHROME_H) * 100 / zoom` and scaled onto the window at the blit, so zoom REFLOWS (the
// host's reflowing zoom) and every page coordinate main.rs computes stays in the page's own CSS px.
//
// INPUT (`input`, called on every ring event before main.rs's dispatch: `None` = the chrome took it, `Some(ev)` =
// main.rs handles `ev`, a pointer event rewritten into page px).
// * TABS (`tabs::TabStrip`): Ctrl+T (0x14) a new tab (the new-tab page: bookmarks, recent history, the volume's
//   pages; the address field focused), Ctrl+W (0x17) closes the active tab (the last one closed opens a fresh
//   new-tab page, the host's rule), Ctrl+1..8 (0x81..0x88, SMALLFIX9's chord bytes) the n-th tab, Ctrl+9 (0x89) the
//   last; a click on a tab selects it, on its × closes it, on `+` opens one. Each tab owns its page (arena + layout),
//   its session history and its scroll: the inactive ones are PARKED here, the active one is main.rs's `page` /
//   `App::hist` / `App::scroll`; a switch swaps them. Ctrl+Tab cannot arrive: the HID fold suppresses every
//   Ctrl+non-letter-non-digit (kernel drivers/xhci/mod.rs `hid_key_ascii`) — the report names the change.
// * SESSION HISTORY (`history::SessionHistory`, per tab, in `App::hist`): Back/Forward buttons beside the keys
//   main.rs already binds (Backspace/Left/button 4, Right/button 5); a fragment link is a same-document entry.
// * OMNIBOX (`omnibox::resolve` + an `edit_core::Buffer`): Ctrl+L (0x0C) or a click focuses it with the URL
//   selected; typing, Backspace, Left/Right, Home/End and the APPCLIP actions (copy, cut, paste, select-all, the
//   caret and selection actions) edit through edit_core (the core the host Tabula links); Enter navigates — a path
//   (`/…`) or a name beside the page opens off the volume, a URL is followed (a non-`file:` URL needs the network,
//   which the image's Aether opens through its net module, else the nav line says `not-a-file-url`), anything else
//   is a search through Principia's `aether.search` template; Esc restores the URL and gives the page its keys back.
// * FIND (`find::find_all`, ui.rs): main.rs's Ctrl+F bar, the matcher now the model's (Unicode simple case fold).
// * BOOKMARKS (`bookmarks::Bookmarks`): Ctrl+D (0x04) or the star toggles the page; the file is the host's
//   `bookmarks.tsv` under the home (store.rs), written on every toggle. HISTORY: every page read is a visit,
//   `history.tsv` beside it, written on every visit.
// * DOWNLOADS (`downloads::*`): a link whose target is not a document Aether renders (`downloads::renderable` on the
//   extension's type) is saved instead of opened, and Ctrl+S (0x13) saves the page: the model's file name
//   (sanitized, made unique against what is there) in `<home>/Desktop`.
// * ZOOM (`zoom::step`, Chromium's ladder): Ctrl+0 (0x80) back to 100 %; Ctrl+plus/minus are suppressed by the same
//   HID fold, so `+`/`=` and `-` (the page has no text input on the image, as `/` for find) and the toolbar's
//   buttons step it.
//
// POINTER (AETHERPTR B543/B547, CURSORSHAPE B550): every MOUSE_ABS is the system pointer in window px. Over the
// chrome the chrome hit-tests it (the hand over a tab or a button, the text beam over the address field) and the
// page hovers nothing; below it the position is rewritten into page px and main.rs's M3 dispatch runs unchanged.
//
// WIRE. `:: AETHER: store home=<h|none> bookmarks=<n> history=<n> code=<rc> ::` at start;
// `:: AETHER: tab -> PASS how=<new|close|select> tabs=<n> active=<k> ::`;
// `:: AETHER: omnibox -> PASS how=<enter|esc> kind=<path|file|url|search> text=<t> ::` (the nav line follows);
// `:: AETHER: zoom -> PASS pct=<p> w=<lw> h=<lh> ::`; `:: AETHER: bookmark -> PASS on=<0|1> n=<n> saved=<rc> ::`;
// `:: AETHER: download -> PASS|FAIL how=<link|save> name=<n> bytes=<n> to=<path> rc=<rc> ::`.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::Cell;
use edit_core::{Buffer, Direction, Motion, Selection, Unit};
use layout_core::FontSel;
use una_abi::{
    input_ev_pack, input_ev_payload, input_ev_type, input_ev_xy, input_ev_xy_payload, CURSOR_ARROW, CURSOR_HAND, CURSOR_TEXT, INPUT_EV_ACTION,
    INPUT_EV_BUTTON, INPUT_EV_KEY_DOWN, INPUT_EV_MOUSE_ABS, INPUT_EV_WIN_RESIZE, KEY_DOWN, KEY_ESC, KEY_LEFT, KEY_RIGHT, KEY_UP,
};

use crate::paint::Canvas;
use crate::sys::sys;
use crate::txt::Fonts;
use crate::{cursor_shape, cut, App, Line, Page, MAX_H, MAX_W};

// ---- the host chrome's model, linked (handlers/aether/src/chrome/*.rs; std-free: core + alloc) ----------------
#[allow(dead_code, unused_imports)]
#[path = "../../../../handlers/aether/src/chrome/tabs.rs"]
pub mod tabs;
#[allow(dead_code, unused_imports)]
#[path = "../../../../handlers/aether/src/chrome/history.rs"]
pub mod history;
#[allow(dead_code, unused_imports)]
#[path = "../../../../handlers/aether/src/chrome/omnibox.rs"]
pub mod omnibox;
#[allow(dead_code, unused_imports)]
#[path = "../../../../handlers/aether/src/chrome/bookmarks.rs"]
pub mod bookmarks;
#[allow(dead_code, unused_imports)]
#[path = "../../../../handlers/aether/src/chrome/find.rs"]
pub mod find;
#[allow(dead_code, unused_imports)]
#[path = "../../../../handlers/aether/src/chrome/downloads.rs"]
pub mod downloads;
#[allow(dead_code, unused_imports)]
#[path = "../../../../handlers/aether/src/chrome/zoom.rs"]
pub mod zoom;

use crate::store;
use history::{SessionHistory, Traversal};

/// The tab strip's height, the toolbar's, and the chrome's (the page starts below it), window px.
pub const TAB_H: i32 = 18;
pub const BAR_H: i32 = 20;
pub const CHROME_H: i32 = TAB_H + BAR_H;
/// The new-tab page's path (generated, never read off the volume).
pub const NEWTAB: &str = "/aether:newtab";
/// The chrome's text size, px.
const FONT: f32 = 11.0;
/// What an empty address field says.
const PLACEHOLDER: &str = "Search or type a path";

/// A tab that is not the active one: its page, its session history, its scroll, and the logical size it was laid out
/// at (a zoom or a resize since then relays it out on return).
struct Parked {
    id: u64,
    page: Page,
    hist: SessionHistory,
    scroll: f32,
    w: i32,
    h: i32,
}

/// The address field while it has the keys.
struct Omni {
    buf: Buffer,
    /// The URL it showed when focused (Esc puts it back).
    saved: String,
    /// The field's horizontal scroll (keeps the caret visible), px.
    scroll: Cell<f32>,
}

/// A place on the chrome the pointer can be over.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hot {
    None,
    Tab(u64),
    Close(u64),
    NewTab,
    Back,
    Fwd,
    Omni,
    Star,
    ZoomOut,
    ZoomIn,
}

pub struct Chrome {
    pub strip: tabs::TabStrip,
    parked: Vec<Parked>,
    /// The window's size (the surface the chrome and the page share) and the zoom, percent.
    pub pw: i32,
    pub ph: i32,
    pub zoom: u32,
    omni: Option<Omni>,
    /// The active page's URL (`file://<path>`; empty on the new-tab page) — what the field shows.
    url: String,
    bookmarks: bookmarks::Bookmarks,
    visits: Vec<(String, String)>,
    downloads: downloads::DownloadList,
    next_dl: u64,
    home: Option<String>,
    search: String,
    /// The pointer in window px, what it is over, and what a held left press started on.
    ptr: (i32, i32),
    hot: Hot,
    pressed: Option<Hot>,
}

impl Chrome {
    pub fn new() -> Chrome {
        let mut strip = tabs::TabStrip::new();
        strip.open();
        Chrome {
            strip,
            parked: Vec::new(),
            pw: MAX_W,
            ph: MAX_H,
            zoom: 100,
            omni: None,
            url: String::new(),
            bookmarks: bookmarks::Bookmarks::new(),
            visits: Vec::new(),
            downloads: downloads::DownloadList::default(),
            next_dl: 1,
            home: None,
            search: String::from(omnibox::DEFAULT_SEARCH),
            ptr: (-1, -1),
            hot: Hot::None,
            pressed: None,
        }
    }

    /// The page's logical size at this window and zoom (CSS px).
    fn logical(&self) -> (i32, i32) {
        let z = self.zoom.max(1) as i32;
        let lw = (self.pw * 100 / z).max(16);
        let lh = ((self.ph - CHROME_H).max(16) * 100 / z).max(16);
        (lw, lh)
    }

    fn over(&self) -> bool {
        let (x, y) = self.ptr;
        x >= 0 && y >= 0 && x < self.pw && y < CHROME_H
    }

    fn file(&self, leaf: &str) -> Option<String> {
        self.home.as_ref().map(|h| alloc::format!("{}{}{}", h, store::DATA_DIR, leaf))
    }

    fn save_bookmarks(&self) -> i64 {
        match self.file(store::BOOKMARKS) {
            Some(p) => store::write(&p, self.bookmarks.serialize().as_bytes()),
            None => una_abi::ENOENT,
        }
    }

    fn save_visits(&self) -> i64 {
        match self.file(store::HISTORY) {
            Some(p) => store::write(&p, store::visits_serialize(&self.visits).as_bytes()),
            None => una_abi::ENOENT,
        }
    }

    /// Give the address field the keys, the URL selected.
    fn focus_omni(&mut self) {
        let t = self.url.clone();
        let mut buf = Buffer::new(&t);
        buf.set_selections(&[Selection::new(0, t.len())]);
        self.omni = Some(Omni { buf, saved: t, scroll: Cell::new(0.0) });
    }
}

/// A point in the page's logical px (window-relative, the scroll already taken off) in WINDOW px — below the chrome,
/// under the zoom: what the focus line's `at=` reports, so `tests aether` aims the system pointer where it lands.
pub fn to_window(app: &App, x: f32, y: f32) -> (i64, i64) {
    let z = app.ch.zoom as f32 / 100.0;
    ((x * z) as i64, (y * z) as i64 + CHROME_H as i64)
}

/// The URL the chrome shows for a page path (`file://` + the path; nothing for the new-tab page).
pub fn url_of(path: &str) -> String {
    if path == NEWTAB { String::new() } else { alloc::format!("file://{}", path) }
}

/// The page's `<title>`, whitespace collapsed.
fn title_of(page: &Page) -> String {
    let doc = page.doc;
    doc.descendants(html_core::Document::ROOT)
        .find(|&n| doc.element(n).is_some_and(|e| e.is_html("title")))
        .map(|n| doc.text_content(n).split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

/// An HTML-escaped copy of `s` for text and double-quoted attributes.
fn esc_attr(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '&' => o.push_str("&amp;"),
            '"' => o.push_str("&quot;"),
            _ => o.push(c),
        }
    }
    o
}

/// The new-tab page: the bookmarks, the most recent visits, the volume's own pages — links the engine lays out
/// and the M3 click follows like any other.
pub fn newtab_html(app: &App) -> String {
    let ch = &app.ch;
    let mut s = String::from(
        "<!doctype html><title>New Tab</title><style>body{margin:8px;font-size:12px}h1{font-size:16px;margin:2px 0 6px}\
h2{font-size:13px;margin:10px 0 2px}ul{margin:0;padding-left:16px}li{margin:1px 0}p{margin:2px 0}</style>\
<h1>Aether</h1><p>Type a path or a search in the address bar (Ctrl+L).</p>",
    );
    let list = |s: &mut String, head: &str, items: &mut dyn Iterator<Item = (String, String)>| {
        let mut any = false;
        for (url, title) in items {
            if !any {
                s.push_str("<h2>");
                s.push_str(head);
                s.push_str("</h2><ul>");
                any = true;
            }
            let label = if title.trim().is_empty() { url.clone() } else { title };
            s.push_str("<li><a href=\"");
            s.push_str(&esc_attr(&url));
            s.push_str("\">");
            s.push_str(&esc_attr(&label));
            s.push_str("</a></li>");
        }
        if any {
            s.push_str("</ul>");
        }
    };
    list(&mut s, "Bookmarks", &mut ch.bookmarks.items().iter().map(|b| (b.url.clone(), b.title.clone())));
    list(&mut s, "Recently visited", &mut ch.visits.iter().rev().take(12).cloned());
    list(
        &mut s,
        "On this volume",
        &mut [("file:///system/doc/welcome.html", "Welcome to Aether"), ("file:///system/doc/keys.html", "Aether keys")]
            .into_iter()
            .map(|(u, t)| (String::from(u), String::from(t))),
    );
    s
}

/// At start: the person's files (store.rs), the search template, the first tab's page.
pub fn start(app: &mut App, page: &Page) {
    let (home, code) = match store::home() {
        Ok(h) => (Some(h), 0),
        Err(e) => (None, e),
    };
    app.ch.home = home;
    let mut code = code;
    if let Some(p) = app.ch.file(store::BOOKMARKS) {
        match store::read_text(&p) {
            Ok(t) => app.ch.bookmarks = bookmarks::Bookmarks::parse(&t),
            Err(e) => code = e,
        }
    }
    if let Some(p) = app.ch.file(store::HISTORY) {
        if let Ok(t) = store::read_text(&p) {
            app.ch.visits = store::visits_parse(&t);
        }
    }
    if let Some(t) = crate::theme::pref_text("aether.search") {
        app.ch.search = t;
    }
    let mut l = Line::new(b":: AETHER: store home=");
    l.put(app.ch.home.as_deref().map(str::as_bytes).unwrap_or(b"none" as &[u8]));
    l.put(b" bookmarks=").dec(app.ch.bookmarks.items().len() as i64).put(b" history=").dec(app.ch.visits.len() as i64);
    l.put(b" code=").dec(code).put(b" ::");
    l.wire();
    loaded(app, page, true);
}

/// A page became the active tab's (main.rs `load`, and a same-document traversal): the tab's title and URL, the
/// visit, the field.
pub fn loaded(app: &mut App, page: &Page, read: bool) {
    let url = url_of(&page.path);
    let title = title_of(page);
    app.hist.set_title(&title);
    let id = app.ch.strip.active();
    app.ch.strip.update(id, Some(&title), Some(&url), Some(false));
    if read && !url.is_empty() {
        store::visit(&mut app.ch.visits, &url, &title);
        let _ = app.ch.save_visits();
    }
    app.ch.url = url;
    app.dirty = true;
}

/// A same-document history step (main.rs): what the active entry's URL is now.
pub fn traverse(app: &mut App, page: &mut Page, fonts: &mut crate::txt::Fonts, delta: i32) {
    let how: &[u8] = if delta < 0 { b"back" } else { b"fwd" };
    match app.hist.go(delta) {
        Some(Traversal::CrossDocument { url }) => {
            let (path, frag) = split_frag(&url);
            crate::load(app, page, fonts, &path, how, false);
            app.hist.commit_traversal_load(&url);
            if let Some(f) = frag {
                crate::scroll_to_frag(app, page, &f);
            }
        }
        Some(Traversal::SameDocument { url, .. }) => {
            let (_, frag) = split_frag(&url);
            let ok = match &frag {
                Some(f) => crate::scroll_to_frag(app, page, f),
                None => {
                    app.scroll = 0.0;
                    app.dirty = true;
                    true
                }
            };
            let mut l = Line::new(b":: AETHER: nav -> ");
            l.put(if ok { b"PASS" as &[u8] } else { b"FAIL reason=no-anchor" }).put(b" how=").put(how);
            l.put(b" path=").put(cut(url.as_bytes(), 72)).put(b" hist=").dec(app.hist.index() as i64 + 1).put(b"/").dec(app.hist.len() as i64).put(b" ::");
            l.wire();
        }
        None => {}
    }
}

/// `path#frag` → (path, frag).
fn split_frag(url: &str) -> (String, Option<String>) {
    match url.split_once('#') {
        Some((p, f)) => (String::from(p), Some(String::from(f))),
        None => (String::from(url), None),
    }
}

// ---- tabs -----------------------------------------------------------------------------------------------------

fn tab_wire(app: &App, how: &[u8]) {
    let ch = &app.ch;
    let k = ch.strip.tabs().iter().position(|t| t.id == ch.strip.active()).map_or(0, |i| i + 1);
    let mut l = Line::new(b":: AETHER: tab -> PASS how=");
    l.put(how).put(b" tabs=").dec(ch.strip.len() as i64).put(b" active=").dec(k as i64).put(b" ::");
    l.wire();
}

/// Move the active tab's state into the parked list, leaving `placeholder` as the page.
fn park(app: &mut App, page: &mut Page, placeholder: Page) {
    let id = app.ch.strip.active();
    let cur = core::mem::replace(page, placeholder);
    let hist = core::mem::take(&mut app.hist);
    app.ch.parked.push(Parked { id, page: cur, hist, scroll: app.scroll, w: app.w, h: app.h });
}

/// Make parked tab `id` the live one (the page in `page` is freed).
fn unpark(app: &mut App, page: &mut Page, id: u64) -> bool {
    let Some(i) = app.ch.parked.iter().position(|p| p.id == id) else { return false };
    let p = app.ch.parked.remove(i);
    let old = core::mem::replace(page, p.page);
    old.free();
    app.hist = p.hist;
    app.find = None;
    app.reset_view();
    app.scroll = p.scroll;
    if p.w != app.w || p.h != app.h {
        app.relayout = true;
    } else {
        app.scroll = app.scroll.clamp(0.0, app.max_scroll(page));
    }
    app.ch.url = url_of(&page.path);
    true
}

fn placeholder(app: &App) -> Page {
    Page::open("", String::from(NEWTAB), app.w, app.h)
}

/// Ctrl+T / `+`: a new tab on the new-tab page, the address field focused.
fn new_tab(app: &mut App, page: &mut Page, fonts: &mut Fonts) {
    let ph = placeholder(app);
    park(app, page, ph);
    app.ch.strip.open();
    app.ch.omni = None;
    crate::load(app, page, fonts, NEWTAB, b"newtab", true);
    app.ch.focus_omni();
    tab_wire(app, b"new");
}

/// Ctrl+W / ×: close the active tab; the strip's rule picks the next (the last one closed opens a fresh one).
fn close_tab(app: &mut App, page: &mut Page, fonts: &mut Fonts) {
    let id = app.ch.strip.active();
    app.ch.omni = None;
    match app.ch.strip.close(id) {
        Some(next) if unpark(app, page, next) => {}
        _ => {
            if app.ch.strip.is_empty() {
                app.ch.strip.open();
            }
            app.hist = SessionHistory::new();
            crate::load(app, page, fonts, NEWTAB, b"newtab", true);
            app.ch.focus_omni();
        }
    }
    app.dirty = true;
    tab_wire(app, b"close");
}

/// Select tab `id` (a click, Ctrl+digit).
fn select_tab(app: &mut App, page: &mut Page, id: u64) {
    if id == app.ch.strip.active() || app.ch.strip.get(id).is_none() {
        return;
    }
    app.ch.omni = None;
    let ph = placeholder(app);
    park(app, page, ph);
    app.ch.strip.select(id);
    if !unpark(app, page, id) {
        // Not parked (cannot happen: every inactive tab is) — take the parked one back rather than show nothing.
        let me = app.ch.parked.last().map(|p| p.id).unwrap_or(0);
        app.ch.strip.select(me);
        unpark(app, page, me);
    }
    app.dirty = true;
    tab_wire(app, b"select");
}

fn select_nth(app: &mut App, page: &mut Page, n: Option<usize>) {
    let tabs = app.ch.strip.tabs();
    let id = match n {
        Some(i) => tabs.get(i).map(|t| t.id),
        None => tabs.last().map(|t| t.id),
    };
    if let Some(id) = id {
        select_tab(app, page, id);
    }
}

// ---- zoom, bookmarks, downloads ---------------------------------------------------------------------------------

/// Re-derive the page's logical size (a zoom step, a window resize): a change relays the page out.
fn refit(app: &mut App) {
    let (lw, lh) = app.ch.logical();
    if lw != app.w || lh != app.h {
        app.w = lw;
        app.h = lh;
        app.relayout = true;
    }
    app.dirty = true;
}

fn zoom_step(app: &mut App, dir: i32) {
    app.ch.zoom = zoom::step(app.ch.zoom, dir);
    refit(app);
    let mut l = Line::new(b":: AETHER: zoom -> PASS pct=");
    l.dec(app.ch.zoom as i64).put(b" w=").dec(app.w as i64).put(b" h=").dec(app.h as i64).put(b" ::");
    l.wire();
    app.flash(alloc::format!("zoom {}%", app.ch.zoom));
}

fn bookmark(app: &mut App, page: &Page) {
    let url = url_of(&page.path);
    if url.is_empty() {
        Line::new(b":: AETHER: bookmark -> FAIL reason=newtab ::").wire();
        return;
    }
    let on = app.ch.bookmarks.toggle(&url, &title_of(page));
    let rc = app.ch.save_bookmarks();
    let mut l = Line::new(b":: AETHER: bookmark -> PASS on=");
    l.dec(on as i64).put(b" n=").dec(app.ch.bookmarks.items().len() as i64).put(b" saved=").dec(rc).put(b" ::");
    l.wire();
    app.flash(String::from(if on { "bookmarked" } else { "bookmark removed" }));
}

/// The MIME type a volume path's extension names, for `downloads::renderable` (no extension: none, rendered).
fn content_type(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or("");
    let ext = name.rsplit_once('.').map(|(_, e)| e)?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "html" | "htm" | "xhtml" | "xht" => "text/html",
        "txt" | "text" | "md" => "text/plain",
        "xml" => "text/xml",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    })
}

/// Whether following a link to `path` saves it rather than opens it.
pub fn is_download(path: &str) -> bool {
    path != NEWTAB && !downloads::renderable(content_type(path))
}

/// Save the volume file `path` in `<home>/Desktop` under the model's name.
pub fn download(app: &mut App, path: &str, how: &[u8]) {
    let url = url_of(path);
    let mut l = Line::new(b":: AETHER: download -> ");
    let fail = |l: &mut Line, why: &[u8], code: i64| {
        l.put(b"FAIL reason=").put(why).put(b" how=").put(how).put(b" path=").put(cut(path.as_bytes(), 64)).put(b" rc=").dec(code).put(b" ::");
        l.wire();
    };
    let Some(home) = app.ch.home.clone() else {
        fail(&mut l, b"no-home", una_abi::ENOENT);
        app.flash(String::from("download: no home (no session)"));
        return;
    };
    let bytes = match crate::fetch(path) {
        Ok((b, _)) => b,
        Err(e) => {
            fail(&mut l, b"unreadable", e);
            app.flash(alloc::format!("download: cannot read {}", path));
            return;
        }
    };
    let dir = alloc::format!("{}{}", home, store::DOWNLOADS);
    let name = downloads::unique_name(&downloads::file_name(None, &url), |n| store::exists(&alloc::format!("{}/{}", dir, n)));
    let to = alloc::format!("{}/{}", dir, name);
    let rc = store::write(&to, &bytes);
    let id = app.ch.next_dl;
    app.ch.next_dl += 1;
    let mut d = downloads::Download::new(id, &name, &url, Some(bytes.len() as u64));
    d.done = true;
    if rc >= 0 {
        d.received = bytes.len() as u64;
    } else {
        d.error = Some(alloc::format!("write refused ({})", rc));
    }
    app.ch.downloads.upsert(d);
    l.put(if rc >= 0 { b"PASS" as &[u8] } else { b"FAIL reason=write-refused" }).put(b" how=").put(how);
    l.put(b" name=").put(cut(name.as_bytes(), 48)).put(b" bytes=").dec(bytes.len() as i64).put(b" to=").put(cut(to.as_bytes(), 72)).put(b" rc=").dec(rc);
    l.put(b" ::");
    l.wire();
    app.flash(if rc >= 0 { alloc::format!("saved {} to Desktop", name) } else { alloc::format!("download refused ({})", rc) });
}

// ---- the omnibox ----------------------------------------------------------------------------------------------

/// The clipboard's text (SYS_CLIP_GET), one line.
fn clip_get() -> Option<String> {
    let mut b = alloc::vec![0u8; una_abi::CLIP_CAP];
    let n = sys(una_abi::SYS_CLIP_GET, b.as_mut_ptr() as u64, b.len() as u64, 0, 0);
    if n <= 0 {
        return None;
    }
    b.truncate(n as usize);
    Some(String::from_utf8_lossy(&b).chars().map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c }).collect())
}

/// Enter in the field: decide (path, a file beside the page, URL, search) and follow it.
fn omni_enter(app: &mut App, page: &mut Page, fonts: &mut Fonts) {
    let Some(o) = app.ch.omni.take() else { return };
    let text = o.buf.text().to_string();
    let t = text.trim();
    app.dirty = true;
    if t.is_empty() {
        return;
    }
    let (kind, target): (&[u8], String) = if t.starts_with('/') {
        (b"path", String::from(t))
    } else {
        match omnibox::resolve(t, &app.ch.search) {
            Some(omnibox::Resolved::Url(u)) => {
                let lower = u.to_ascii_lowercase();
                let has_scheme = lower.contains("://") || ["about:", "data:", "file:", "mailto:", "blob:", "view-source:"].iter().any(|s| lower.starts_with(s));
                // A scheme-less name: a file beside the page when the volume has one (`keys.html`), else the web.
                let beside = if has_scheme { None } else { crate::resolve(&page.path, &u).ok().map(|(p, _)| p).filter(|p| crate::fetch(p).is_ok()) };
                match beside {
                    Some(_) => (b"file", u),
                    None if has_scheme => (b"url", u),
                    None => (b"url", alloc::format!("https://{}", u)),
                }
            }
            Some(omnibox::Resolved::Search(u)) => (b"search", u),
            None => return,
        }
    };
    let mut l = Line::new(b":: AETHER: omnibox -> PASS how=enter kind=");
    l.put(kind).put(b" text=").put(cut(t.as_bytes(), 64)).put(b" ::");
    l.wire();
    crate::follow(app, page, fonts, &target, b"omnibox");
}

/// A key while the field has the keys.
fn omni_key(app: &mut App, page: &mut Page, fonts: &mut Fonts, c: u8) {
    app.dirty = true;
    match c {
        KEY_ESC => {
            if let Some(o) = app.ch.omni.take() {
                Line::new(b":: AETHER: omnibox -> PASS how=esc text=").put(cut(o.saved.as_bytes(), 64)).put(b" ::").wire();
            }
        }
        b'\n' | b'\r' => omni_enter(app, page, fonts),
        _ => {
            let Some(o) = app.ch.omni.as_mut() else { return };
            let b = &mut o.buf;
            match c {
                0x08 | 0x7F => {
                    b.delete(Unit::Grapheme, Direction::Backward);
                }
                KEY_LEFT => b.move_cursors(Motion::Left, false),
                KEY_RIGHT => b.move_cursors(Motion::Right, false),
                KEY_UP => b.move_cursors(Motion::DocStart, false),
                KEY_DOWN => b.move_cursors(Motion::DocEnd, false),
                0x01 | 0x0C => {
                    let n = b.len();
                    b.set_selections(&[Selection::new(0, n)]);
                }
                0x05 => b.move_cursors(Motion::DocEnd, false),
                0x15 => {
                    let n = b.len();
                    b.replace(0..n, "");
                }
                0x20..=0x7E => {
                    let mut s = [0u8; 1];
                    s[0] = c;
                    b.insert(core::str::from_utf8(&s).unwrap_or(""));
                }
                _ => {}
            }
        }
    }
}

/// An APPCLIP action while the field has the keys (una-abi INPUT_EV_ACTION's numbers).
fn omni_action(app: &mut App, a: u64) {
    let Some(o) = app.ch.omni.as_mut() else { return };
    let b = &mut o.buf;
    let sel = b.selections()[0];
    match a {
        3 | 4 => {
            if !sel.is_empty() {
                let t = b.slice(sel.range());
                let t = crate::ui::clip_text(&t);
                let _ = sys(una_abi::SYS_CLIP_SET, t.as_ptr() as u64, t.len() as u64, 0, 0);
                if a == 4 {
                    b.delete(Unit::Grapheme, Direction::Backward);
                }
            }
        }
        5 => {
            if let Some(t) = clip_get() {
                b.insert(&t);
            }
        }
        6 => {
            let n = b.len();
            b.set_selections(&[Selection::new(0, n)]);
        }
        8 => b.move_cursors(Motion::Left, true),
        9 => b.move_cursors(Motion::Right, true),
        10 => b.move_cursors(Motion::DocStart, true),
        11 => b.move_cursors(Motion::DocEnd, true),
        12 => {
            let h = sel.head;
            b.set_cursor(h);
        }
        13 => b.move_cursors(Motion::Left, false),
        14 => b.move_cursors(Motion::Right, false),
        15 => b.move_cursors(Motion::DocStart, false),
        16 => b.move_cursors(Motion::DocEnd, false),
        _ => {}
    }
    app.dirty = true;
}

// ---- geometry, hit test, input ----------------------------------------------------------------------------------

/// The chrome's rectangles at the current window width (x, w) — tabs in the strip, buttons and the field in the bar.
struct Geo {
    tabs: Vec<(u64, i32, i32)>,
    plus: (i32, i32),
    back: (i32, i32),
    fwd: (i32, i32),
    omni: (i32, i32),
    star: (i32, i32),
    zout: (i32, i32),
    zin: (i32, i32),
}

const BTN: i32 = 18;
const CLOSE_W: i32 = 12;

fn geo(ch: &Chrome) -> Geo {
    let w = ch.pw;
    let area = (w - BTN - 4).max(24);
    let n = ch.strip.len().max(1) as i32;
    let tw = (area / n).clamp(24, 112);
    // Tabs past the strip's room are not drawn (Ctrl+1..9 still reach them).
    let tabs: Vec<(u64, i32, i32)> =
        ch.strip.tabs().iter().enumerate().map(|(i, t)| (t.id, 2 + i as i32 * tw, tw - 2)).filter(|&(_, x, tw)| x + tw <= area + 2).collect();
    let plus_x = (2 + tabs.len() as i32 * tw).min(w - BTN - 2);
    Geo {
        tabs,
        plus: (plus_x, BTN),
        back: (2, BTN),
        fwd: (2 + BTN + 2, BTN),
        omni: (2 * BTN + 8, (w - 2 * BTN - 8 - 3 * (BTN + 2) - 4).max(24)),
        star: (w - 3 * (BTN + 2), BTN),
        zout: (w - 2 * (BTN + 2), BTN),
        zin: (w - (BTN + 2), BTN),
    }
}

fn within(x: i32, r: (i32, i32)) -> bool {
    x >= r.0 && x < r.0 + r.1
}

fn hit(ch: &Chrome, x: i32, y: i32) -> Hot {
    if y < 0 || y >= CHROME_H || x < 0 || x >= ch.pw {
        return Hot::None;
    }
    let g = geo(ch);
    if y < TAB_H {
        for &(id, tx, tw) in &g.tabs {
            if within(x, (tx, tw)) {
                if id == ch.strip.active() && x >= tx + tw - CLOSE_W - 2 {
                    return Hot::Close(id);
                }
                return Hot::Tab(id);
            }
        }
        return if within(x, g.plus) { Hot::NewTab } else { Hot::None };
    }
    for (r, h) in [(g.back, Hot::Back), (g.fwd, Hot::Fwd), (g.omni, Hot::Omni), (g.star, Hot::Star), (g.zout, Hot::ZoomOut), (g.zin, Hot::ZoomIn)] {
        if within(x, r) {
            return h;
        }
    }
    Hot::None
}

/// The byte offset in `text` nearest x (field px from the text's origin).
fn caret_at(fonts: &Fonts, text: &str, x: f32) -> usize {
    let mut t = fonts.0.borrow_mut();
    let mut best = (0usize, f32::MAX);
    for (i, _) in text.char_indices().chain(core::iter::once((text.len(), ' '))) {
        let d = (t.measure(&text[..i], FontSel::default(), FONT) - x).abs();
        if d < best.1 {
            best = (i, d);
        }
    }
    best.0
}

/// A left click released on `h` (the press started on it too).
fn click(app: &mut App, page: &mut Page, fonts: &mut Fonts, h: Hot) {
    match h {
        Hot::Tab(id) => select_tab(app, page, id),
        Hot::Close(_) => close_tab(app, page, fonts),
        Hot::NewTab => new_tab(app, page, fonts),
        Hot::Back => crate::back(app, page, fonts),
        Hot::Fwd => crate::forward(app, page, fonts),
        Hot::Star => bookmark(app, page),
        Hot::ZoomOut => zoom_step(app, -1),
        Hot::ZoomIn => zoom_step(app, 1),
        Hot::Omni | Hot::None => {}
    }
}

/// Every ring event, before main.rs's dispatch: `None` = the chrome took it; `Some(ev)` = main.rs handles `ev`.
pub fn input(app: &mut App, page: &mut Page, fonts: &mut Fonts, ev: u64) -> Option<u64> {
    let p = input_ev_payload(ev);
    match input_ev_type(ev) {
        INPUT_EV_KEY_DOWN => {
            let c = p as u8;
            // The tab chords work from the field too; every other key is the field's while it has the keys.
            if app.ch.omni.is_some() && !matches!(c, 0x14 | 0x17 | 0x81..=0x89) {
                omni_key(app, page, fonts, c);
                return None;
            }
            match c {
                0x14 => new_tab(app, page, fonts),
                0x17 => close_tab(app, page, fonts),
                0x0C => {
                    app.ch.focus_omni();
                    app.dirty = true;
                }
                0x04 => bookmark(app, page),
                0x13 => {
                    if page.path == NEWTAB {
                        Line::new(b":: AETHER: download -> FAIL reason=newtab how=save ::").wire();
                    } else {
                        let path = page.path.clone();
                        download(app, &path, b"save");
                    }
                }
                0x80 => zoom_step(app, 0),
                0x81..=0x88 => select_nth(app, page, Some((c - 0x81) as usize)),
                0x89 => select_nth(app, page, None),
                b'+' | b'=' if app.find.is_none() => zoom_step(app, 1),
                b'-' if app.find.is_none() => zoom_step(app, -1),
                _ => return Some(ev),
            }
            None
        }
        INPUT_EV_ACTION if app.ch.omni.is_some() => {
            omni_action(app, p);
            None
        }
        INPUT_EV_WIN_RESIZE => {
            app.ch.pw = (((p >> 16) & 0xffff) as i32).clamp(64, MAX_W);
            app.ch.ph = ((p & 0xffff) as i32).clamp(CHROME_H + 32, MAX_H);
            refit(app);
            None
        }
        INPUT_EV_MOUSE_ABS => {
            let (nx, ny) = input_ev_xy(p);
            app.ch.ptr = (nx, ny);
            let z = app.ch.zoom.max(1) as i32;
            let (lx, ly) = ((nx * 100).div_euclid(z), ((ny - CHROME_H) * 100).div_euclid(z));
            let held = app.pressed || app.buttons & 4 != 0;
            if app.ch.over() && !held {
                let h = hit(&app.ch, nx, ny);
                if h != app.ch.hot {
                    app.ch.hot = h;
                    app.dirty = true;
                }
                cursor_shape(match h {
                    Hot::Omni => CURSOR_TEXT,
                    Hot::None => CURSOR_ARROW,
                    _ => CURSOR_HAND,
                });
                if app.hover.take().is_some() {
                    app.dirty = true;
                }
                app.raw = Some((lx, ly));
                return None;
            }
            if app.ch.hot != Hot::None {
                app.ch.hot = Hot::None;
                app.dirty = true;
            }
            Some(input_ev_pack(INPUT_EV_MOUSE_ABS, input_ev_xy_payload(lx, ly)))
        }
        INPUT_EV_BUTTON => {
            let (m, was) = (p as u8, app.buttons);
            if m & 1 != 0 && was & 1 == 0 && app.ch.over() {
                app.buttons = m;
                let h = hit(&app.ch, app.ch.ptr.0, app.ch.ptr.1);
                app.ch.pressed = Some(h);
                if h == Hot::Omni {
                    // A press in the field focuses it (URL selected) or, when it has the keys, places the caret.
                    let g = geo(&app.ch);
                    if let Some(o) = app.ch.omni.as_mut() {
                        let text = o.buf.text().to_string();
                        let x = (app.ch.ptr.0 - g.omni.0 - 3) as f32 + o.scroll.get();
                        let at = caret_at(fonts, &text, x);
                        o.buf.set_cursor(at);
                    } else {
                        app.ch.focus_omni();
                    }
                } else if app.ch.omni.is_some() {
                    app.ch.omni = None; // a press elsewhere on the chrome gives the page its keys back
                }
                app.dirty = true;
                return None;
            }
            if m & 1 == 0 && was & 1 != 0 && app.ch.pressed.is_some() {
                app.buttons = m;
                let start = app.ch.pressed.take().unwrap_or(Hot::None);
                let now = hit(&app.ch, app.ch.ptr.0, app.ch.ptr.1);
                if start == now {
                    click(app, page, fonts, now);
                }
                return None;
            }
            if m & 1 != 0 && was & 1 == 0 && app.ch.omni.is_some() {
                app.ch.omni = None; // a press on the page blurs the field (its text goes back to the URL)
                app.dirty = true;
            }
            Some(ev)
        }
        _ => Some(ev),
    }
}

// ---- paint and blit ---------------------------------------------------------------------------------------------

/// Draw `s` at x on the band [top, top + h), clipped to [x0, x1); returns the pen's end.
#[allow(clippy::too_many_arguments)]
fn label(cv: &mut Canvas, fonts: &Fonts, s: &str, x: f32, top: i32, h: i32, ink: u32, x0: i32, x1: i32) -> f32 {
    let sel = FontSel::default();
    let mut t = fonts.0.borrow_mut();
    let (a, d, _) = t.line_metrics(sel, FONT);
    let base = top as f32 + ((h as f32 + a - d) / 2.0);
    let base = font_core::fmath::floor(base);
    let (cw, chh) = (cv.w, cv.h);
    t.draw(x, base, s, sel, FONT, ink, &mut |gx, gy, cov| {
        if gx >= x0 && gx < x1 && gx >= 0 && gy >= 0 && gx < cw && gy < chh {
            let i = (gy * cw + gx) as usize;
            cv.px[i] = font_core::ui::blend(cv.px[i], ink, cov);
        }
    });
    x + t.measure(s, sel, FONT)
}

/// `s` cut (with an ellipsis) to fit `w` px.
fn fit(fonts: &Fonts, s: &str, w: f32) -> String {
    let mut t = fonts.0.borrow_mut();
    if t.measure(s, FontSel::default(), FONT) <= w {
        return String::from(s);
    }
    let mut out = String::new();
    for c in s.chars() {
        out.push(c);
        if t.measure(&out, FontSel::default(), FONT) + t.measure("\u{2026}", FontSel::default(), FONT) > w {
            out.pop();
            break;
        }
    }
    out.push('\u{2026}');
    out
}

/// A button: its hover wash and its glyph centred, dimmed when it can do nothing.
fn button(cv: &mut Canvas, fonts: &Fonts, r: (i32, i32), glyph: &str, hot: bool, enabled: bool, ink: u32) {
    let th = crate::theme::cur();
    let (top, h) = (TAB_H + 1, BAR_H - 2);
    if hot && enabled {
        cv.fill(r.0, top, r.1, h, th.hot_bg);
    }
    let gw = fonts.0.borrow_mut().measure(glyph, FontSel::default(), FONT);
    let x = r.0 as f32 + (r.1 as f32 - gw) / 2.0;
    label(cv, fonts, glyph, x, top, h, if enabled { ink } else { th.chrome_dim }, r.0, r.0 + r.1);
}

/// Paint the strip and the toolbar into `cv` (the window's width, CHROME_H tall).
fn paint_chrome(app: &App, cv: &mut Canvas, fonts: &Fonts) {
    let ch = &app.ch;
    let th = crate::theme::cur();
    let g = geo(ch);
    cv.fill(0, 0, cv.w, cv.h, th.chrome_bg);
    // The strip.
    for (i, &(id, tx, tw)) in g.tabs.iter().enumerate() {
        let active = id == ch.strip.active();
        let bg = if active {
            th.tab_active
        } else if ch.hot == Hot::Tab(id) {
            th.hot_bg
        } else {
            th.tab_bg
        };
        cv.fill(tx, 2, tw, TAB_H - 2, bg);
        if active {
            cv.fill(tx, 2, tw, 2, th.focus);
        }
        let lab = ch.strip.tabs()[i].label().to_string();
        let room = (tw - 8 - if active { CLOSE_W + 2 } else { 0 }) as f32;
        let s = fit(fonts, &lab, room.max(8.0));
        label(cv, fonts, &s, (tx + 4) as f32, 3, TAB_H - 3, if active { th.chrome_ink } else { th.chrome_dim }, tx, tx + tw - if active { CLOSE_W + 2 } else { 2 });
        if active {
            let cx = tx + tw - CLOSE_W - 2;
            if ch.hot == Hot::Close(id) {
                cv.fill(cx, 4, CLOSE_W, TAB_H - 6, th.hot_bg);
            }
            label(cv, fonts, "\u{00D7}", (cx + 3) as f32, 3, TAB_H - 3, th.chrome_ink, cx, cx + CLOSE_W);
        }
    }
    if ch.hot == Hot::NewTab {
        cv.fill(g.plus.0, 3, g.plus.1, TAB_H - 4, th.hot_bg);
    }
    let pw = fonts.0.borrow_mut().measure("+", FontSel::default(), FONT);
    label(cv, fonts, "+", g.plus.0 as f32 + (g.plus.1 as f32 - pw) / 2.0, 3, TAB_H - 3, th.chrome_ink, g.plus.0, g.plus.0 + g.plus.1);
    cv.fill(0, TAB_H, cv.w, 1, th.chrome_rule);
    // The toolbar.
    let ink = th.chrome_ink;
    button(cv, fonts, g.back, "<", ch.hot == Hot::Back, app.hist.can_go(-1), ink);
    button(cv, fonts, g.fwd, ">", ch.hot == Hot::Fwd, app.hist.can_go(1), ink);
    let on = !ch.url.is_empty() && ch.bookmarks.contains(&ch.url);
    button(cv, fonts, g.star, if on { "\u{2605}" } else { "\u{2606}" }, ch.hot == Hot::Star, !ch.url.is_empty(), if on { th.focus } else { ink });
    button(cv, fonts, g.zout, "-", ch.hot == Hot::ZoomOut, ch.zoom > zoom::LEVELS[0], ink);
    button(cv, fonts, g.zin, "+", ch.hot == Hot::ZoomIn, ch.zoom < zoom::LEVELS[zoom::LEVELS.len() - 1], ink);
    // The address field.
    let (fx, fw) = g.omni;
    let (ft, fh) = (TAB_H + 2, BAR_H - 4);
    cv.fill(fx, ft, fw, fh, th.field_bg);
    crate::paint::outline(cv, fx, ft, fw, fh, if ch.omni.is_some() { th.focus } else { th.chrome_rule });
    let mut x1 = fx + fw - 3;
    if ch.zoom != 100 {
        let z = alloc::format!("{}%", ch.zoom);
        let zw = fonts.0.borrow_mut().measure(&z, FontSel::default(), FONT);
        let zx = (fx + fw - 4) as f32 - zw;
        label(cv, fonts, &z, zx, ft, fh, th.chrome_dim, fx, fx + fw - 2);
        x1 = zx as i32 - 3;
    }
    let tx0 = fx + 3;
    match &ch.omni {
        Some(o) => {
            let text = o.buf.text().to_string();
            let sel = o.buf.selections()[0];
            let (caret, s0, s1) = {
                let mut t = fonts.0.borrow_mut();
                let m = |t: &mut crate::txt::Txt, b: usize| t.measure(&text[..b.min(text.len())], FontSel::default(), FONT);
                (m(&mut *t, sel.head), m(&mut *t, sel.start()), m(&mut *t, sel.end()))
            };
            let vw = (x1 - tx0).max(8) as f32;
            let mut sc = o.scroll.get();
            if caret - sc > vw - 2.0 {
                sc = caret - vw + 2.0;
            }
            if caret < sc {
                sc = caret;
            }
            o.scroll.set(sc.max(0.0));
            let ox = tx0 as f32 - sc;
            if !sel.is_empty() {
                let (a, b) = ((ox + s0) as i32, (ox + s1) as i32);
                let (a, b) = (a.max(tx0), b.min(x1));
                if b > a {
                    cv.fill(a, ft + 2, b - a, fh - 4, th.field_sel);
                }
            }
            label(cv, fonts, &text, ox, ft, fh, th.field_ink, tx0, x1);
            let cx = (ox + caret) as i32;
            if cx >= tx0 && cx <= x1 {
                cv.fill(cx, ft + 2, 1, fh - 4, th.focus);
            }
        }
        None if ch.url.is_empty() => {
            label(cv, fonts, PLACEHOLDER, tx0 as f32, ft, fh, th.chrome_dim, tx0, x1);
        }
        None => {
            let s = fit(fonts, &ch.url, (x1 - tx0) as f32);
            label(cv, fonts, &s, tx0 as f32, ft, fh, th.field_ink, tx0, x1);
        }
    }
    cv.fill(0, CHROME_H - 1, cv.w, 1, th.chrome_rule);
}

/// One frame onto the surface: the chrome on top, the page canvas (logical px) scaled onto the rest — a straight
/// copy at 100 %, bilinear otherwise — then SYS_WIN_PRESENT.
pub fn blit(app: &App, cv: &Canvas, fonts: &Fonts) {
    let (pw, ph) = (app.ch.pw.min(MAX_W), app.ch.ph.min(MAX_H));
    let mut top = Canvas::new(pw, CHROME_H);
    paint_chrome(app, &mut top, fonts);
    let put = |x: i32, y: i32, c: u32| {
        let row = unsafe { app.surf.add(y as usize * crate::STRIDE) as *mut u32 };
        unsafe { row.add(x as usize).write_volatile(0xFF00_0000 | c) };
    };
    for y in 0..CHROME_H.min(ph) {
        for x in 0..pw {
            put(x, y, top.px[(y * pw + x) as usize]);
        }
    }
    let dh = ph - CHROME_H;
    if dh > 0 && cv.w > 0 && cv.h > 0 {
        if cv.w == pw && cv.h == dh {
            for y in 0..dh {
                for x in 0..pw {
                    put(x, CHROME_H + y, cv.px[(y * cv.w + x) as usize]);
                }
            }
        } else {
            // 16.16 fixed point: the source sample centred under each window pixel.
            let sx = |x: i32| -> (i32, i32, u32) {
                let f = (((2 * x + 1) as i64 * cv.w as i64 * 65536) / (2 * pw as i64) - 32768).max(0);
                let x0 = ((f >> 16) as i32).min(cv.w - 1);
                (x0, (x0 + 1).min(cv.w - 1), ((f >> 8) & 0xFF) as u32)
            };
            let xs: Vec<(i32, i32, u32)> = (0..pw).map(sx).collect();
            for y in 0..dh {
                let f = (((2 * y + 1) as i64 * cv.h as i64 * 65536) / (2 * dh as i64) - 32768).max(0);
                let y0 = ((f >> 16) as i32).min(cv.h - 1);
                let y1 = (y0 + 1).min(cv.h - 1);
                let fy = ((f >> 8) & 0xFF) as u32;
                for (x, &(x0, x1, fx)) in xs.iter().enumerate() {
                    let px = |xx: i32, yy: i32| cv.px[(yy * cv.w + xx) as usize];
                    let (c00, c10, c01, c11) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
                    let mut out = 0u32;
                    let mut s = 0;
                    while s < 24 {
                        let ch = |c: u32| (c >> s) & 0xFF;
                        let top = ch(c00) * (256 - fx) + ch(c10) * fx;
                        let bot = ch(c01) * (256 - fx) + ch(c11) * fx;
                        let v = (top * (256 - fy) + bot * fy) >> 16;
                        out |= v.min(255) << s;
                        s += 8;
                    }
                    put(x as i32, CHROME_H + y, out);
                }
            }
        }
    }
    sys(una_abi::SYS_WIN_PRESENT, app.win as u64, 0, 0, 0);
}
