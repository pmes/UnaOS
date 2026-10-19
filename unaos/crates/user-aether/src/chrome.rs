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
// HEADER CRAFT (SR140): every dimension is one named constant (THE HEADER'S METRICS) and every colour one `Skin`
// field derived from the theme tokens. The toolbar is the active tab's ground (they join with no seam); the strip is
// a half-step darker with hairlines between quiet tabs; a tab shows its letter tile and title centred and its × on
// hover; the toolbar's controls are stroked glyphs with a rounded wash on hover and press, never a boxed rectangle;
// the address field is one pill with a quiet border and, focused, the accent inside a soft halo.
//
// SIDE TABS AND THE HEADER'S ACTIONS (AETHERSIDETABS2, SR143). Principia's `aether.tabs.layout` (`top` | `side`, the
// host face's key; read at start and whenever `theme::due` reports a PrefChanged — its subscription names `aether.tabs.`
// beside `system.appearance.`; a kernel that refuses it falls back to due's own poll) chooses where the tabs stand. `side`: the model's
// `sidetabs::SideStrip` (linked by path like the rest) is a COLUMN down the window's left — `width_for(pw)`: 240 px,
// at most a third of this 288 px surface, or the 48 px rail — over its toggle row and `TabStrip::rows` 32 px each
// (a group's header with its colour dot, name, count and chevron, a click collapsing it; its tabs with a stripe in
// the group's colour; the favicon or the letter tile, the title, the × on hover; the `+` row), scrolled by the
// wheel and revealed on every tab switch; the toolbar then spans only the page's width, and the PAGE'S ORIGIN is
// (column width, BAR_H) instead of (0, CHROME_H): `page_x`/`page_y` below are the one place it is decided, and
// `logical`, `to_window`, the pointer rewrite and `blit` all read them. The toolbar gains RELOAD (the page read
// again, history untouched: main.rs `load` with push = false) and the MENU (⋮ at the right end): the chrome's
// actions as a list — new tab, bookmark this page, bookmarks and history (the new-tab page lists both), downloads,
// find in page, zoom in / out / actual size, settings. On a narrow bar the zoom buttons (under 360 px) and the star
// (under 220 px) leave the bar for the menu. A tab's FAVICON tile (`sidetabs::favicon_tile`, 16 px) replaces its
// letter tile when the face has one (`Chrome::set_icon`).
//
// INPUT (`input`, called on every ring event before main.rs's dispatch: `None` = the chrome took it, `Some(ev)` =
// main.rs handles `ev`, a pointer event rewritten into page px).
// * TABS (`tabs::TabStrip`): Ctrl+T (0x14) a new tab (the new-tab page: bookmarks, recent history, the volume's
//   pages; the address field focused), Ctrl+W (0x17) closes the active tab (the last one closed opens a fresh
//   new-tab page, the host's rule), Ctrl+1..8 (0x81..0x88, SMALLFIX9's chord bytes) the n-th tab, Ctrl+9 (0x89) the
//   last, Ctrl+Tab (0x8A, una-abi KEY_CTRL_TAB: AETHERMETAL5's chord byte out of the kernel's HID fold) the next tab and
//   Ctrl+Shift+Tab (0x8B) the previous, wrapping; a click on a tab selects it, on its × closes it, on `+` opens one.
//   Each tab owns its page (arena + layout), its session history and its scroll: the inactive ones are PARKED here,
//   the active one is main.rs's `page` / `App::hist` / `App::scroll`; a switch swaps them.
// * SESSION HISTORY (`history::SessionHistory`, per tab, in `App::hist`): Back/Forward buttons beside the keys
//   main.rs already binds (Backspace/Left/button 4, Right/button 5); a fragment link is a same-document entry.
// * OMNIBOX (`omnibox::resolve` + an `edit_core::Buffer`): Ctrl+L (0x0C) or a click focuses it with the URL
//   selected; typing, Backspace, Left/Right, Home/End and the APPCLIP actions (copy, cut, paste, select-all, the
//   caret and selection actions) edit through edit_core (the core the host Tabula links); Enter navigates — a path
//   (`/…`) or a name beside the page opens off the volume, a URL is followed (an http(s) URL — typed, or a bare name
//   completed to `https://` — through AETHERNET's `net::fetch`, SR110, its `:: AETHER: net` line before the nav
//   line), anything else is a search through Principia's `aether.search` template, fetched the same way; Esc restores the URL and gives the page its keys back.
// * FIND (`find::find_all`, ui.rs): main.rs's Ctrl+F bar, the matcher now the model's (Unicode simple case fold).
// * BOOKMARKS (`bookmarks::Bookmarks`): Ctrl+D (0x04) or the star toggles the page; the file is the host's
//   `bookmarks.tsv` under the home (store.rs), written on every toggle. HISTORY: every page read is a visit,
//   `history.tsv` beside it, written on every visit.
// * DOWNLOADS (`downloads::*`): a link whose target is not a document Aether renders (`downloads::renderable` on the
//   extension's type) is saved instead of opened, and Ctrl+S (0x13) saves the page: the model's file name
//   (sanitized, made unique against what is there) in `<home>/Desktop`.
// * ZOOM (`zoom::step`, Chromium's ladder): Ctrl+0 (0x80) back to 100 %, Ctrl+plus (0x8C, Ctrl+`=` with or without
//   Shift) and Ctrl+minus (0x8D) step it, as do bare `+`/`=` and `-` (the page has no text input on the image, as
//   `/` for find) and the toolbar's buttons. Every chord byte (0x80..=0x8D) works from the address field too.
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
    INPUT_EV_BUTTON, INPUT_EV_KEY_DOWN, INPUT_EV_MOUSE_ABS, INPUT_EV_WHEEL, INPUT_EV_WIN_RESIZE, KEY_CTRL_MINUS, KEY_CTRL_PLUS, KEY_CTRL_SHIFT_TAB, KEY_CTRL_TAB,
    KEY_DOWN, KEY_ESC, KEY_LEFT, KEY_RIGHT, KEY_UP,
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
// AETHERSIDETABS2 (SR143): the side column's geometry, the layout pref and the favicon tile (std-free; names only
// `super::tabs`).
#[allow(dead_code, unused_imports)]
#[path = "../../../../handlers/aether/src/chrome/sidetabs.rs"]
pub mod sidetabs;

use crate::store;
use history::{SessionHistory, Traversal};
use sidetabs::{SideHit, SideStrip, TabLayout};

// ---- THE HEADER'S METRICS (AETHERSIDETABS / header craft, SR140): every dimension the strip and the toolbar are
// drawn with, named once here; the colours are `Skin` (below), derived once from the theme tokens. ----------------
/// The tab strip's band, the toolbar's band, and the chrome's height (the page starts below it), window px.
pub const TAB_H: i32 = 26;
pub const BAR_H: i32 = 30;
pub const CHROME_H: i32 = TAB_H + BAR_H;
/// The type: the BODY size (tab titles, the address field) and one step down (the zoom badge, the letter tile).
const FONT: f32 = 11.0;
const FONT_SMALL: f32 = 9.5;
/// The header's inset from the window's left and right edges, and the gap between neighbouring controls.
const EDGE: i32 = 8;
const GAP: i32 = 4;
/// A tab: the air above it in the strip, its width range, its top corners' radius, its inner padding (both sides).
const TAB_TOP: i32 = 4;
const TAB_MIN_W: i32 = 40;
const TAB_MAX_W: i32 = 128;
const TAB_R: i32 = 6;
const TAB_PAD: i32 = 8;
/// The hairline between two inactive tabs: its inset from the tab body's top and bottom.
const SEP_INSET: i32 = 7;
/// The favicon slot (the letter tile until the engine hands a favicon over), its radius, and the gap to the title.
const TILE: i32 = 14;
const TILE_R: i32 = 4;
const TILE_GAP: i32 = 6;
/// The close affordance (a circle, shown while the tab is hovered) and the × inside it (half its arm).
const CLOSE: i32 = 16;
const CLOSE_ARM: f32 = 3.0;
/// A toolbar control's square (the field's height too), its hover / pressed wash radius.
const CTL: i32 = 22;
const CTL_R: i32 = 6;
/// The address field: its radius (a full pill), its text inset, the focus ring's width.
const FIELD_R: i32 = CTL / 2;
const FIELD_PAD: i32 = 10;
const RING: i32 = 2;
/// The drawn glyphs (chevrons, plus, minus, ×): half their extent and their stroke width.
const GLYPH: f32 = 4.0;
const STROKE: f32 = 1.5;
/// Where the toolbar's controls stand in the bar (their top edge below the bar's top; `Chrome::bar_y`).
const BAR_INSET: i32 = (BAR_H - CTL) / 2;
/// AETHERSIDETABS2 (SR143): the narrowest bar that still carries the zoom buttons, and the star (else they are only
/// in the menu).
const BAR_ZOOM_MIN: i32 = 360;
const BAR_STAR_MIN: i32 = 220;
/// The menu: its width, one row's height, its inner padding, its radius, and its rows.
const MENU_W: i32 = 176;
const MENU_ROW: i32 = 22;
const MENU_PAD: i32 = 4;
const MENU_R: i32 = 8;
const MENU: [(&str, &str); 10] = [
    ("New tab", "Ctrl+T"),
    ("Bookmark this page", "Ctrl+D"),
    ("Bookmarks and history", ""),
    ("Downloads", ""),
    ("Find in page", "Ctrl+F"),
    ("Zoom in", "Ctrl++"),
    ("Zoom out", "Ctrl+-"),
    ("Actual size", "Ctrl+0"),
    ("Close tab", "Ctrl+W"),
    ("Settings", ""),
];
/// The side column's favicon / letter tile (the model's `FAVICON_PX`) and the group's colour dot.
const SIDE_TILE: i32 = sidetabs::FAVICON_PX as i32;
const DOT: i32 = 8;
/// Principia's row for where the tabs stand (`sidetabs::PREF_NS` . `sidetabs::PREF_KEY`).
const LAYOUT_PREF: &str = "aether.tabs.layout";
/// The new-tab page's path (generated, never read off the volume).
pub const NEWTAB: &str = "/aether:newtab";
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
    /// AETHERSIDETABS2 (SR143): reload, the menu button, one of its rows, and a place in the side column.
    Reload,
    Menu,
    MenuItem(usize),
    Side(SideHit),
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
    /// AETHERSIDETABS2 (SR143): where the tabs stand (Principia's row) and the side column's state, whether the menu
    /// is open, and each tab's favicon tile (16 x 16 RGBA) when the face has one.
    layout: TabLayout,
    side: SideStrip,
    menu: bool,
    icons: Vec<(u64, Vec<u8>)>,
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
            layout: TabLayout::Top,
            side: SideStrip::default(),
            menu: false,
            icons: Vec::new(),
        }
    }

    fn is_side(&self) -> bool {
        self.layout == TabLayout::Side
    }

    /// The side column's width (0 in the top layout).
    fn col_w(&self) -> i32 {
        if self.is_side() {
            self.side.width_for(self.pw.max(0) as u32) as i32
        } else {
            0
        }
    }

    /// THE PAGE'S ORIGIN in window px (AETHERSIDETABS2): right of the side column and under the toolbar in the side
    /// layout, under the whole header (0, CHROME_H) in the top one.
    pub fn page_x(&self) -> i32 {
        self.col_w()
    }

    pub fn page_y(&self) -> i32 {
        if self.is_side() {
            BAR_H
        } else {
            CHROME_H
        }
    }

    /// The toolbar's band top, and its controls' top edge.
    fn bar_top(&self) -> i32 {
        self.page_y() - BAR_H
    }

    fn bar_y(&self) -> i32 {
        self.bar_top() + BAR_INSET
    }

    /// The page's logical size at this window and zoom (CSS px).
    fn logical(&self) -> (i32, i32) {
        let z = self.zoom.max(1) as i32;
        let lw = ((self.pw - self.page_x()).max(16) * 100 / z).max(16);
        let lh = ((self.ph - self.page_y()).max(16) * 100 / z).max(16);
        (lw, lh)
    }

    /// The open menu's rectangle (x, y, w, h), under the menu button's right edge.
    fn menu_rect(&self) -> (i32, i32, i32, i32) {
        let w = MENU_W.min(self.pw - 2 * EDGE);
        let h = MENU.len() as i32 * MENU_ROW + 2 * MENU_PAD;
        (self.pw - EDGE - w, self.page_y() + 2, w, h)
    }

    fn in_menu(&self, x: i32, y: i32) -> bool {
        let (mx, my, mw, mh) = self.menu_rect();
        self.menu && x >= mx && x < mx + mw && y >= my && y < my + mh
    }

    fn over(&self) -> bool {
        let (x, y) = self.ptr;
        x >= 0 && y >= 0 && x < self.pw && y < self.ph && (y < self.page_y() || x < self.page_x() || self.in_menu(x, y))
    }

    /// AETHERSIDETABS2 (SR143): tab `id`'s favicon tile (`sidetabs::FAVICON_PX` square RGBA, what
    /// `sidetabs::favicon_tile` makes of a decoded `<link rel=icon>`), or `None` to show its letter tile again.
    pub fn set_icon(&mut self, id: u64, tile: Option<Vec<u8>>) {
        self.icons.retain(|(k, _)| *k != id);
        if let Some(t) = tile.filter(|t| t.len() >= (SIDE_TILE * SIDE_TILE * 4) as usize) {
            self.icons.push((id, t));
        }
    }

    fn icon(&self, id: u64) -> Option<&[u8]> {
        self.icons.iter().find(|(k, _)| *k == id).map(|(_, t)| t.as_slice())
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
    ((x * z) as i64 + app.ch.page_x() as i64, (y * z) as i64 + app.ch.page_y() as i64)
}

/// The URL the chrome shows for a page path (`file://` + the path; a network page's path is its URL already; nothing
/// for the new-tab page).
pub fn url_of(path: &str) -> String {
    if path == NEWTAB {
        String::new()
    } else if crate::net::is_net(path) {
        String::from(path)
    } else {
        alloc::format!("file://{}", path)
    }
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
    prefs(app, true); // AETHERSIDETABS2 (SR143): where the tabs stand, before the first paint
    loaded(app, page, true);
}

/// AETHERSIDETABS2 (SR143): re-read where the tabs stand — at start, whenever `theme::due` saw the store move
/// (`moved`: a PrefChanged under `aether.tabs.` or `system.appearance.`, theme.rs's subscription). A
/// change re-lays the window (the page's origin and logical size move; no tab or page is touched):
/// `:: AETHER: tabs layout=<top|side> col=<px> ::`.
pub fn prefs(app: &mut App, moved: bool) {
    if !moved {
        return;
    }
    let want = crate::theme::pref_text(LAYOUT_PREF).map(|t| TabLayout::parse(&t)).unwrap_or_default();
    if want == app.ch.layout {
        return;
    }
    app.ch.layout = want;
    app.ch.hot = Hot::None;
    app.ch.menu = false;
    let (strip, ph, id) = (&app.ch.strip, app.ch.ph.max(0) as u32, app.ch.strip.active());
    app.ch.side.reveal(strip, ph, id);
    refit(app);
    let mut l = Line::new(b":: AETHER: tabs layout=");
    l.put(want.as_str().as_bytes()).put(b" col=").dec(app.ch.col_w() as i64).put(b" ::");
    l.wire();
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

fn tab_wire(app: &mut App, how: &[u8]) {
    // AETHERSIDETABS2 (SR143): the side column keeps the active tab on view; a closed tab's favicon goes.
    {
        let ch = &mut app.ch;
        let (ph, id) = (ch.ph.max(0) as u32, ch.strip.active());
        ch.side.reveal(&ch.strip, ph, id);
        let strip = &ch.strip;
        ch.icons.retain(|(k, _)| strip.get(*k).is_some());
    }
    let ch = &app.ch;
    let k = ch.strip.tabs().iter().position(|t| t.id == ch.strip.active()).map_or(0, |i| i + 1);
    let mut l = Line::new(b":: AETHER: tab -> PASS how=");
    l.put(how).put(b" tabs=").dec(ch.strip.len() as i64).put(b" active=").dec(k as i64).put(b" ::");
    l.wire();
}

/// Move tab `id`'s state (the live one) into the parked list, leaving `placeholder` as the page.
fn park(app: &mut App, page: &mut Page, placeholder: Page, id: u64) {
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
    let id = app.ch.strip.active();
    park(app, page, ph, id);
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
    let live = app.ch.strip.active();
    park(app, page, ph, live);
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

/// Ctrl+Tab / Ctrl+Shift+Tab: the tab `dir` places along the strip, wrapping (`TabStrip::step`'s rule, applied
/// through `select_tab` so the pages swap).
fn cycle_tab(app: &mut App, page: &mut Page, dir: i32) {
    let tabs = app.ch.strip.tabs();
    let Some(i) = tabs.iter().position(|t| t.id == app.ch.strip.active()) else { return };
    let j = (i as i64 + dir as i64).rem_euclid(tabs.len() as i64) as usize;
    let id = tabs[j].id;
    select_tab(app, page, id);
}

/// AETHERSIDETABS2 (SR143): a click on a group's header collapses or expands it. Collapsing the active tab's group
/// moves the active tab out of it (the model's rule), so the pages swap as a switch does.
fn toggle_group(app: &mut App, page: &mut Page, g: tabs::GroupId) {
    let before = app.ch.strip.active();
    if app.ch.strip.group_toggle(g).is_none() {
        return;
    }
    let now = app.ch.strip.active();
    if now != before {
        app.ch.omni = None;
        let ph = placeholder(app);
        park(app, page, ph, before);
        unpark(app, page, now);
        tab_wire(app, b"select");
    }
    app.dirty = true;
}

/// AETHERSIDETABS2 (SR143): the toolbar's reload — the page read again in place (main.rs `load`, no new history
/// entry): `:: AETHER: nav -> PASS how=reload ... ::`.
fn reload(app: &mut App, page: &mut Page, fonts: &mut Fonts) {
    app.ch.omni = None;
    let path = page.path.clone();
    crate::load(app, page, fonts, &path, b"reload", false);
}

/// AETHERSIDETABS2 (SR143): one of the menu's rows (`MENU`'s order).
fn menu_item(app: &mut App, page: &mut Page, fonts: &mut Fonts, i: usize) {
    app.ch.menu = false;
    app.dirty = true;
    match i {
        0 => new_tab(app, page, fonts),
        1 => bookmark(app, page),
        2 => crate::load(app, page, fonts, NEWTAB, b"menu", true),
        3 => {
            let d = &app.ch.downloads;
            let msg = match d.items().last() {
                None => String::from("no downloads yet"),
                Some(x) => alloc::format!("{} download(s); last {} ({})", d.items().len(), x.name, if x.error.is_some() { "failed" } else { "in Desktop" }),
            };
            app.flash(msg);
        }
        4 => app.find = Some(crate::ui::Find::default()),
        5 => zoom_step(app, 1),
        6 => zoom_step(app, -1),
        7 => zoom_step(app, 0),
        8 => close_tab(app, page, fonts),
        _ => app.flash(alloc::format!("settings: Principia {} = {}, aether.search", LAYOUT_PREF, app.ch.layout.as_str())),
    }
    let mut l = Line::new(b":: AETHER: menu -> PASS item=");
    l.put(MENU.get(i).map_or(b"none" as &[u8], |m| m.0.as_bytes())).put(b" ::");
    l.wire();
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

/// Whether following a link to `path` saves it rather than opens it. A network URL always opens: its type is the
/// server's, not its last path segment's (`?q=rust+1.80`, `/page.php`), and net::fetch reads the page either way.
pub fn is_download(path: &str) -> bool {
    path != NEWTAB && !crate::net::is_net(path) && !downloads::renderable(content_type(path))
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
                // Only a VOLUME file is probed: beside a network page the name would resolve to a URL, and probing it would be
                // a navigation of its own.
                let beside = if has_scheme { None } else { crate::resolve(&page.path, &u).ok().map(|(p, _)| p).filter(|p| !crate::net::is_net(p) && crate::fetch(p).is_ok()) };
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

/// The chrome's rectangles at the current window width (x, w) — tabs in the strip (the top layout only), buttons and
/// the field in the bar (which spans the page's width: right of the side column in the side layout). A control the bar
/// has no room for is `None` (it is in the menu).
struct Geo {
    tabs: Vec<(u64, i32, i32)>,
    plus: (i32, i32),
    back: (i32, i32),
    fwd: (i32, i32),
    reload: (i32, i32),
    omni: (i32, i32),
    star: Option<(i32, i32)>,
    zout: Option<(i32, i32)>,
    zin: Option<(i32, i32)>,
    menu: (i32, i32),
}

fn geo(ch: &Chrome) -> Geo {
    let w = ch.pw;
    let (tabs, plus) = if ch.is_side() {
        (Vec::new(), (0, 0))
    } else {
        let area = (w - 2 * EDGE - GAP - CTL).max(TAB_MIN_W);
        let n = ch.strip.len().max(1) as i32;
        let tw = (area / n).clamp(TAB_MIN_W, TAB_MAX_W);
        // Tabs past the strip's room are not drawn (Ctrl+1..9 still reach them).
        let tabs: Vec<(u64, i32, i32)> =
            ch.strip.tabs().iter().enumerate().map(|(i, t)| (t.id, EDGE + i as i32 * tw, tw)).filter(|&(_, x, tw)| x + tw <= EDGE + area).collect();
        let plus_x = (EDGE + tabs.len() as i32 * tw + GAP).min(w - EDGE - CTL);
        (tabs, (plus_x, CTL))
    };
    let x0 = ch.page_x();
    let bw = w - x0;
    let menu = w - EDGE - CTL;
    let mut r = menu - GAP;
    let (zout, zin) = if bw >= BAR_ZOOM_MIN {
        let zin = r - CTL;
        let zout = zin - GAP - CTL;
        r = zout - GAP;
        (Some((zout, CTL)), Some((zin, CTL)))
    } else {
        (None, None)
    };
    let star = if bw >= BAR_STAR_MIN {
        r -= CTL;
        let s = r;
        r -= GAP;
        Some((s, CTL))
    } else {
        None
    };
    let back = x0 + EDGE;
    let fwd = back + CTL + GAP;
    let reload = fwd + CTL + GAP;
    let omni = reload + CTL + EDGE;
    Geo {
        tabs,
        plus,
        back: (back, CTL),
        fwd: (fwd, CTL),
        reload: (reload, CTL),
        omni: (omni, (r + GAP - EDGE - omni).max(24)),
        star,
        zout,
        zin,
        menu: (menu, CTL),
    }
}

/// Where a tab's close affordance stands (its left edge), for a tab at `tx` that is `tw` wide.
fn close_x(tx: i32, tw: i32) -> i32 {
    tx + tw - TAB_PAD / 2 - CLOSE
}

fn within(x: i32, r: (i32, i32)) -> bool {
    x >= r.0 && x < r.0 + r.1
}

fn hit(ch: &Chrome, x: i32, y: i32) -> Hot {
    if ch.menu {
        // The open menu sits over the page: its rows are the chrome's.
        let (mx, my, mw, mh) = ch.menu_rect();
        if x >= mx && x < mx + mw && y >= my && y < my + mh {
            let k = (y - my - MENU_PAD).div_euclid(MENU_ROW);
            return if k >= 0 && (k as usize) < MENU.len() { Hot::MenuItem(k as usize) } else { Hot::None };
        }
    }
    if y < 0 || y >= ch.ph || x < 0 || x >= ch.pw {
        return Hot::None;
    }
    let cw = ch.col_w();
    if x < cw {
        return ch.side.hit_w(&ch.strip, cw as u32, ch.ph.max(0) as u32, x, y).map_or(Hot::None, Hot::Side);
    }
    if y >= ch.page_y() {
        return Hot::None;
    }
    let g = geo(ch);
    if !ch.is_side() && y < TAB_H {
        if y < TAB_TOP {
            return Hot::None;
        }
        for &(id, tx, tw) in &g.tabs {
            if within(x, (tx, tw)) {
                if x >= close_x(tx, tw) && x < close_x(tx, tw) + CLOSE {
                    return Hot::Close(id);
                }
                return Hot::Tab(id);
            }
        }
        return if within(x, g.plus) { Hot::NewTab } else { Hot::None };
    }
    let by = ch.bar_y();
    if y < by || y >= by + CTL {
        return Hot::None;
    }
    let opt = [(g.star, Hot::Star), (g.zout, Hot::ZoomOut), (g.zin, Hot::ZoomIn)];
    for (r, h) in [(g.back, Hot::Back), (g.fwd, Hot::Fwd), (g.reload, Hot::Reload), (g.omni, Hot::Omni), (g.menu, Hot::Menu)]
        .into_iter()
        .chain(opt.into_iter().filter_map(|(r, h)| r.map(|r| (r, h))))
    {
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
        Hot::Close(id) => {
            // The × shows on whichever tab the pointer is over: that tab is the one closed.
            if id != app.ch.strip.active() {
                select_tab(app, page, id);
            }
            if id == app.ch.strip.active() {
                close_tab(app, page, fonts);
            }
        }
        Hot::NewTab => new_tab(app, page, fonts),
        Hot::Back => crate::back(app, page, fonts),
        Hot::Fwd => crate::forward(app, page, fonts),
        Hot::Star => bookmark(app, page),
        Hot::ZoomOut => zoom_step(app, -1),
        Hot::ZoomIn => zoom_step(app, 1),
        // AETHERSIDETABS2 (SR143): reload, the menu, and the side column.
        Hot::Reload => reload(app, page, fonts),
        Hot::Menu => app.ch.menu = !app.ch.menu,
        Hot::MenuItem(i) => menu_item(app, page, fonts, i),
        Hot::Side(SideHit::Toggle) => {
            app.ch.side.toggle();
            app.ch.hot = Hot::None;
            refit(app);
        }
        Hot::Side(SideHit::Tab(id)) => select_tab(app, page, id),
        Hot::Side(SideHit::Close(id)) => click(app, page, fonts, Hot::Close(id)),
        Hot::Side(SideHit::Group(g)) => toggle_group(app, page, g),
        Hot::Side(SideHit::NewTab) => new_tab(app, page, fonts),
        Hot::Omni | Hot::None => {}
    }
}

/// Every ring event, before main.rs's dispatch: `None` = the chrome took it; `Some(ev)` = main.rs handles `ev`.
pub fn input(app: &mut App, page: &mut Page, fonts: &mut Fonts, ev: u64) -> Option<u64> {
    let p = input_ev_payload(ev);
    match input_ev_type(ev) {
        INPUT_EV_KEY_DOWN if app.ch.menu && p as u8 == KEY_ESC => {
            app.ch.menu = false; // AETHERSIDETABS2 (SR143): Esc closes the menu first
            app.dirty = true;
            None
        }
        INPUT_EV_KEY_DOWN => {
            let c = p as u8;
            // The tab chords and every chord byte (0x80..=0x8D) work from the field too; every other key is the field's while it has the keys.
            if app.ch.omni.is_some() && !matches!(c, 0x14 | 0x17 | 0x80..=0x8D) {
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
                KEY_CTRL_TAB => cycle_tab(app, page, 1),
                KEY_CTRL_SHIFT_TAB => cycle_tab(app, page, -1),
                KEY_CTRL_PLUS => zoom_step(app, 1),
                KEY_CTRL_MINUS => zoom_step(app, -1),
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
            app.ch.side.scroll = app.ch.side.scroll.clamp(0, SideStrip::max_scroll(&app.ch.strip, app.ch.ph as u32));
            refit(app);
            None
        }
        // AETHERSIDETABS2 (SR143): the wheel over the side column scrolls the tab list (positive = toward the top).
        INPUT_EV_WHEEL if app.ch.ptr.0 >= 0 && app.ch.ptr.0 < app.ch.col_w() => {
            let n = p as u8 as i8 as i32;
            let ch = &mut app.ch;
            ch.side.scroll_by(&ch.strip, ch.ph.max(0) as u32, -n * sidetabs::ROW_H as i32);
            ch.hot = hit(ch, ch.ptr.0, ch.ptr.1);
            app.dirty = true;
            None
        }
        INPUT_EV_MOUSE_ABS => {
            let (nx, ny) = input_ev_xy(p);
            app.ch.ptr = (nx, ny);
            let z = app.ch.zoom.max(1) as i32;
            let (ox, oy) = (app.ch.page_x(), app.ch.page_y()); // AETHERSIDETABS2: the page's origin
            let (lx, ly) = (((nx - ox) * 100).div_euclid(z), ((ny - oy) * 100).div_euclid(z));
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
            if m & 1 != 0 && was & 1 == 0 && app.ch.menu && !app.ch.in_menu(app.ch.ptr.0, app.ch.ptr.1) && hit(&app.ch, app.ch.ptr.0, app.ch.ptr.1) != Hot::Menu {
                // AETHERSIDETABS2 (SR143): a press outside the open menu closes it and is spent.
                app.buttons = m;
                app.ch.menu = false;
                app.dirty = true;
                return None;
            }
            if m & 1 != 0 && was & 1 == 0 && app.ch.over() {
                app.buttons = m;
                let h = hit(&app.ch, app.ch.ptr.0, app.ch.ptr.1);
                app.ch.pressed = Some(h);
                if h == Hot::Omni {
                    // A press in the field focuses it (URL selected) or, when it has the keys, places the caret.
                    let g = geo(&app.ch);
                    if let Some(o) = app.ch.omni.as_mut() {
                        let text = o.buf.text().to_string();
                        let x = (app.ch.ptr.0 - g.omni.0 - FIELD_PAD) as f32 + o.scroll.get();
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
                app.dirty = true; // the pressed wash goes
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

/// The header's colours, derived once per frame from the theme tokens (`theme::Theme`, both looks): the toolbar is
/// the active tab's ground (the content fill), so the two join with no seam; the strip is a half-step darker; every
/// wash, hairline and ring is a mix of those with the ink or the accent — one accent, nothing else.
struct Skin {
    strip: u32,
    bar: u32,
    rule: u32,
    sep: u32,
    tab_hover: u32,
    ctl_hover: u32,
    ctl_press: u32,
    ink: u32,
    dim: u32,
    accent: u32,
    tile: u32,
    tile_dim: u32,
    field_bg: u32,
    field_line: u32,
    field_ink: u32,
    field_sel: u32,
    halo: u32,
    /// AETHERSIDETABS2 (SR143): the side column's selected row — the accent at a sixth over the column (§3.10: a
    /// quieter selection than Settings', because many tabs are scanned).
    side_sel: u32,
}

fn skin() -> Skin {
    use palette_core::mix;
    let th = crate::theme::cur();
    let bar = th.tab_active;
    let strip = mix(bar, 0x000000, 1, 12);
    Skin {
        strip,
        bar,
        rule: mix(bar, th.chrome_rule, 1, 2),
        sep: mix(strip, th.chrome_ink, 1, 6),
        tab_hover: mix(strip, th.chrome_ink, 1, 14),
        ctl_hover: mix(bar, th.chrome_ink, 1, 12),
        ctl_press: mix(bar, th.chrome_ink, 1, 6),
        ink: th.chrome_ink,
        dim: th.chrome_dim,
        accent: th.focus,
        tile: mix(bar, th.focus, 3, 4),
        tile_dim: mix(strip, th.chrome_dim, 2, 3),
        field_bg: th.field_bg,
        field_line: mix(bar, th.chrome_ink, 1, 6),
        field_ink: th.field_ink,
        field_sel: th.field_sel,
        halo: mix(bar, th.focus, 2, 5),
        side_sel: mix(strip, th.focus, 1, 6),
    }
}

/// A filled rectangle with anti-aliased round corners (radius `r`; `top_only`: the bottom corners square — a tab).
/// Corner pixels are covered 4 x 4 sub-samples deep (no square root: a sample is in when its squared distance to the
/// corner's centre is within the radius's square).
#[allow(clippy::too_many_arguments)]
fn rrect(cv: &mut Canvas, x: i32, y: i32, w: i32, h: i32, r: i32, c: u32, top_only: bool) {
    if w <= 0 || h <= 0 {
        return;
    }
    let r = r.min(w / 2).min(h / 2).max(0);
    let (x1, y1) = (x + w, y + h);
    for py in y.max(0)..y1.min(cv.h) {
        for px in x.max(0)..x1.min(cv.w) {
            // The corner this pixel falls in, if any: its centre in 1/8 px.
            let cx = if px < x + r { Some(x + r) } else if px >= x1 - r { Some(x1 - r) } else { None };
            let cy = if py < y + r { Some(y + r) } else if py >= y1 - r && !top_only { Some(y1 - r) } else { None };
            let i = (py * cv.w + px) as usize;
            match (cx, cy) {
                (Some(cx), Some(cy)) => {
                    let (cx8, cy8, r8) = (cx * 8, cy * 8, r * 8);
                    let mut n = 0u32;
                    for sy in 0..4 {
                        for sx in 0..4 {
                            let dx = px * 8 + 1 + sx * 2 - cx8;
                            let dy = py * 8 + 1 + sy * 2 - cy8;
                            if dx * dx + dy * dy <= r8 * r8 {
                                n += 1;
                            }
                        }
                    }
                    if n > 0 {
                        cv.px[i] = font_core::ui::blend(cv.px[i], c, (n * 255 / 16) as u8);
                    }
                }
                _ => cv.px[i] = c,
            }
        }
    }
}

/// An anti-aliased stroke from (x0, y0) to (x1, y1), `wd` px wide with round ends (4 x 4 sub-samples a pixel).
fn seg(cv: &mut Canvas, x0: f32, y0: f32, x1: f32, y1: f32, wd: f32, c: u32) {
    let rr = wd * wd / 4.0;
    let (dx, dy) = (x1 - x0, y1 - y0);
    let len2 = (dx * dx + dy * dy).max(1e-6);
    let m = wd / 2.0 + 1.0;
    let (bx0, bx1) = (font_core::fmath::floor(x0.min(x1) - m) as i32, font_core::fmath::ceil(x0.max(x1) + m) as i32);
    let (by0, by1) = (font_core::fmath::floor(y0.min(y1) - m) as i32, font_core::fmath::ceil(y0.max(y1) + m) as i32);
    for py in by0.max(0)..by1.min(cv.h) {
        for px in bx0.max(0)..bx1.min(cv.w) {
            let mut n = 0u32;
            for sy in 0..4 {
                for sx in 0..4 {
                    let (qx, qy) = (px as f32 + (sx as f32 + 0.5) / 4.0, py as f32 + (sy as f32 + 0.5) / 4.0);
                    let t = (((qx - x0) * dx + (qy - y0) * dy) / len2).clamp(0.0, 1.0);
                    let (ex, ey) = (x0 + t * dx - qx, y0 + t * dy - qy);
                    if ex * ex + ey * ey <= rr {
                        n += 1;
                    }
                }
            }
            if n > 0 {
                let i = (py * cv.w + px) as usize;
                cv.px[i] = font_core::ui::blend(cv.px[i], c, (n * 255 / 16) as u8);
            }
        }
    }
}

/// The glyphs the toolbar and the strip draw as strokes (the font's `<` / `+` read as text, not as controls).
#[derive(Clone, Copy)]
enum Glyph {
    Back,
    Fwd,
    Plus,
    Minus,
    Cross,
    /// AETHERSIDETABS2 (SR143): reload (an open circle with its arrowhead), the menu (three dots), a disclosure
    /// chevron pointing down (an expanded group).
    Reload,
    Dots,
    Down,
}

/// The unit circle every 30 degrees (x right, y down), for the reload glyph's arc — no trigonometry in a no_std face.
const RING12: [(f32, f32); 12] = [
    (1.0, 0.0),
    (0.866, 0.5),
    (0.5, 0.866),
    (0.0, 1.0),
    (-0.5, 0.866),
    (-0.866, 0.5),
    (-1.0, 0.0),
    (-0.866, -0.5),
    (-0.5, -0.866),
    (0.0, -1.0),
    (0.5, -0.866),
    (0.866, -0.5),
];

/// Draw `g` centred on (cx, cy).
fn glyph(cv: &mut Canvas, g: Glyph, cx: f32, cy: f32, c: u32) {
    let a = GLYPH;
    match g {
        Glyph::Back => {
            seg(cv, cx + a / 2.0, cy - a, cx - a / 2.0, cy, STROKE, c);
            seg(cv, cx - a / 2.0, cy, cx + a / 2.0, cy + a, STROKE, c);
        }
        Glyph::Fwd => {
            seg(cv, cx - a / 2.0, cy - a, cx + a / 2.0, cy, STROKE, c);
            seg(cv, cx + a / 2.0, cy, cx - a / 2.0, cy + a, STROKE, c);
        }
        Glyph::Plus => {
            seg(cv, cx - a, cy, cx + a, cy, STROKE, c);
            seg(cv, cx, cy - a, cx, cy + a, STROKE, c);
        }
        Glyph::Minus => seg(cv, cx - a, cy, cx + a, cy, STROKE, c),
        Glyph::Cross => {
            let k = CLOSE_ARM;
            seg(cv, cx - k, cy - k, cx + k, cy + k, STROKE, c);
            seg(cv, cx - k, cy + k, cx + k, cy - k, STROKE, c);
        }
        Glyph::Reload => {
            // From the top (-90 deg) clockwise round to -30 deg: ten 30-degree chords, the gap at the upper right,
            // the arrowhead at the arc's end pointing along it.
            let r = a + 0.5;
            let pt = |k: usize| (cx + RING12[k % 12].0 * r, cy + RING12[k % 12].1 * r);
            for k in 9..19 {
                let ((x0, y0), (x1, y1)) = (pt(k), pt(k + 1));
                seg(cv, x0, y0, x1, y1, STROKE, c);
            }
            let (ex, ey) = pt(19);
            // The arc travels up and to the right there; the two barbs lean back from the tip, 45 degrees each side.
            seg(cv, ex, ey, ex - 2.9, ey + 0.8, STROKE, c);
            seg(cv, ex, ey, ex + 0.8, ey + 2.9, STROKE, c);
        }
        Glyph::Dots => {
            for k in -1..=1 {
                let y = cy + k as f32 * (a - 0.5);
                seg(cv, cx, y, cx, y, STROKE + 1.0, c);
            }
        }
        Glyph::Down => {
            seg(cv, cx - a / 1.5, cy - a / 3.0, cx, cy + a / 3.0, STROKE, c);
            seg(cv, cx, cy + a / 3.0, cx + a / 1.5, cy - a / 3.0, STROKE, c);
        }
    }
}

/// Draw `s` at x, vertically centred on the band [top, top + h), clipped to [x0, x1), at `size`; returns the pen's end.
#[allow(clippy::too_many_arguments)]
fn label_at(cv: &mut Canvas, fonts: &Fonts, s: &str, x: f32, top: i32, h: i32, ink: u32, x0: i32, x1: i32, size: f32) -> f32 {
    let sel = FontSel::default();
    let mut t = fonts.0.borrow_mut();
    let (a, d, _) = t.line_metrics(sel, size);
    let base = top as f32 + ((h as f32 + a - d) / 2.0);
    let base = font_core::fmath::floor(base);
    let (cw, chh) = (cv.w, cv.h);
    t.draw(x, base, s, sel, size, ink, &mut |gx, gy, cov| {
        if gx >= x0 && gx < x1 && gx >= 0 && gy >= 0 && gx < cw && gy < chh {
            let i = (gy * cw + gx) as usize;
            cv.px[i] = font_core::ui::blend(cv.px[i], ink, cov);
        }
    });
    x + t.measure(s, sel, size)
}

/// [`label_at`] at the body size.
#[allow(clippy::too_many_arguments)]
fn label(cv: &mut Canvas, fonts: &Fonts, s: &str, x: f32, top: i32, h: i32, ink: u32, x0: i32, x1: i32) -> f32 {
    label_at(cv, fonts, s, x, top, h, ink, x0, x1, FONT)
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

/// A control's state for its wash: pressed (the press started on it and the pointer is still on it), hovered, or
/// at rest.
fn wash(ch: &Chrome, h: Hot, sk: &Skin) -> Option<u32> {
    if ch.hot != h {
        None
    } else if ch.pressed == Some(h) {
        Some(sk.ctl_press)
    } else {
        Some(sk.ctl_hover)
    }
}

/// A toolbar control at `r` (x, w) on the bar: no box at rest, a rounded wash on hover / press, its glyph centred and
/// dimmed when it can do nothing.
fn control(cv: &mut Canvas, ch: &Chrome, sk: &Skin, r: (i32, i32), h: Hot, enabled: bool, g: Glyph) {
    let by = ch.bar_y();
    if enabled {
        if let Some(c) = wash(ch, h, sk) {
            rrect(cv, r.0, by, r.1, CTL, CTL_R, c, false);
        }
    }
    let (cx, cy) = (r.0 as f32 + r.1 as f32 / 2.0, by as f32 + CTL as f32 / 2.0);
    glyph(cv, g, cx, cy, if enabled { sk.ink } else { sk.dim });
}

/// AETHERSIDETABS2 (SR143): a tab's favicon tile (`sidetabs::FAVICON_PX` square RGBA) drawn `s` px square at (x, y),
/// alpha-blended (nearest sample when `s` differs from the tile).
fn draw_icon(cv: &mut Canvas, x: i32, y: i32, s: i32, tile: &[u8]) {
    let n = SIDE_TILE;
    for dy in 0..s {
        for dx in 0..s {
            let (px, py) = (x + dx, y + dy);
            if px < 0 || py < 0 || px >= cv.w || py >= cv.h {
                continue;
            }
            let (sx, sy) = (dx * n / s, dy * n / s);
            let k = ((sy * n + sx) * 4) as usize;
            let Some(q) = tile.get(k..k + 4) else { continue };
            if q[3] == 0 {
                continue;
            }
            let c = (q[0] as u32) << 16 | (q[1] as u32) << 8 | q[2] as u32;
            let i = (py * cv.w + px) as usize;
            cv.px[i] = font_core::ui::blend(cv.px[i], c, q[3]);
        }
    }
}

/// The favicon slot for tab `t` at (x, y), `s` px square: its favicon when the face has one, else the letter tile.
#[allow(clippy::too_many_arguments)]
fn tab_tile(cv: &mut Canvas, ch: &Chrome, fonts: &Fonts, sk: &Skin, t: &tabs::Tab, on: bool, x: i32, y: i32, s: i32) {
    if let Some(icon) = ch.icon(t.id) {
        draw_icon(cv, x, y, s, icon);
        return;
    }
    rrect(cv, x, y, s, s, TILE_R, if on { sk.tile } else { sk.tile_dim }, false);
    let mut letter = [0u8; 4];
    let letter: &str = t.initial().encode_utf8(&mut letter);
    let lw = fonts.0.borrow_mut().measure(letter, FontSel::default(), FONT_SMALL);
    label_at(cv, fonts, letter, x as f32 + (s as f32 - lw) / 2.0, y, s, sk.bar, x, x + s, FONT_SMALL);
}

/// AETHERSIDETABS2 (SR143): the side column — `SideStrip::layout`'s rows under the toggle row, the column's width
/// `Chrome::col_w`, the whole window tall.
fn paint_side(ch: &Chrome, cv: &mut Canvas, fonts: &Fonts, sk: &Skin) {
    let (cw, h) = (ch.col_w(), ch.ph);
    let rh = sidetabs::ROW_H as i32;
    let rail = ch.side.collapsed;
    let active = ch.strip.active();
    cv.fill(0, 0, cw, h, sk.strip);
    for r in ch.side.layout(&ch.strip, h.max(0) as u32) {
        let y = r.y;
        let (bx, bw) = (4, cw - 8);
        match r.row {
            tabs::Row::Tab(id) => {
                let Some(t) = ch.strip.get(id) else { continue };
                let on = id == active;
                let hov = ch.hot == Hot::Side(SideHit::Tab(id)) || ch.hot == Hot::Side(SideHit::Close(id));
                if on {
                    rrect(cv, bx, y + 2, bw, rh - 4, CTL_R, sk.side_sel, false);
                } else if hov {
                    rrect(cv, bx, y + 2, bw, rh - 4, CTL_R, sk.tab_hover, false);
                }
                // A grouped tab's stripe in its group's colour.
                if let Some(g) = t.group.and_then(|g| ch.strip.group(g)) {
                    if !rail {
                        rrect(cv, bx + 2, y + 9, 3, rh - 18, 1, g.color.rgb(), false);
                    }
                }
                let tx = if rail { (cw - SIDE_TILE) / 2 } else { bx + 10 };
                let ty = y + (rh - SIDE_TILE) / 2;
                tab_tile(cv, ch, fonts, sk, t, on, tx, ty, SIDE_TILE);
                if rail {
                    continue;
                }
                let cbox = sidetabs::CLOSE_W as i32;
                let t0 = tx + SIDE_TILE + TILE_GAP;
                let t1 = if hov { cw - cbox } else { bx + bw - 6 };
                if t1 > t0 + 4 {
                    let s = fit(fonts, t.label(), (t1 - t0) as f32);
                    label(cv, fonts, &s, t0 as f32, y, rh, if on { sk.ink } else { sk.dim }, t0, t1);
                }
                if hov {
                    let (cx, cy) = (cw - cbox + (cbox - CLOSE) / 2 - 2, y + (rh - CLOSE) / 2);
                    if ch.hot == Hot::Side(SideHit::Close(id)) {
                        let c = if ch.pressed == Some(ch.hot) { sk.ctl_press } else { sk.ctl_hover };
                        rrect(cv, cx, cy, CLOSE, CLOSE, CLOSE / 2, c, false);
                    }
                    glyph(cv, Glyph::Cross, cx as f32 + CLOSE as f32 / 2.0, cy as f32 + CLOSE as f32 / 2.0, sk.ink);
                }
            }
            tabs::Row::Group(gid) => {
                let Some(g) = ch.strip.group(gid) else { continue };
                if ch.hot == Hot::Side(SideHit::Group(gid)) {
                    rrect(cv, bx, y + 4, bw, rh - 8, CTL_R, sk.tab_hover, false);
                }
                let dx = if rail { (cw - DOT) / 2 } else { bx + 10 + (SIDE_TILE - DOT) / 2 };
                rrect(cv, dx, y + (rh - DOT) / 2, DOT, DOT, DOT / 2, g.color.rgb(), false);
                if rail {
                    continue;
                }
                let name = if g.name.trim().is_empty() { g.color.name() } else { g.name.as_str() };
                let count = alloc::format!("{}", ch.strip.group_tabs(gid).len());
                let cx = (cw - 8 - CTL / 2) as f32;
                glyph(cv, if g.collapsed { Glyph::Fwd } else { Glyph::Down }, cx, (y + rh / 2) as f32, sk.dim);
                let nw = fonts.0.borrow_mut().measure(&count, FontSel::default(), FONT_SMALL);
                let nx = cx - CTL as f32 / 2.0 - nw;
                label_at(cv, fonts, &count, nx, y, rh, sk.dim, 0, cw, FONT_SMALL);
                let t0 = bx + 10 + SIDE_TILE + TILE_GAP;
                let t1 = nx as i32 - GAP;
                if t1 > t0 + 4 {
                    let s = fit(fonts, name, (t1 - t0) as f32);
                    label(cv, fonts, &s, t0 as f32, y, rh, sk.ink, t0, t1);
                }
            }
            tabs::Row::NewTab => {
                if ch.hot == Hot::Side(SideHit::NewTab) {
                    rrect(cv, bx, y + 2, bw, rh - 4, CTL_R, sk.tab_hover, false);
                }
                let tx = if rail { (cw - SIDE_TILE) / 2 } else { bx + 10 };
                glyph(cv, Glyph::Plus, tx as f32 + SIDE_TILE as f32 / 2.0, (y + rh / 2) as f32, sk.dim);
                if !rail {
                    let t0 = tx + SIDE_TILE + TILE_GAP;
                    label(cv, fonts, "New tab", t0 as f32, y, rh, sk.dim, t0, bx + bw);
                }
            }
        }
    }
    // The toggle row, over whatever scrolled under it: collapse (open) or expand (rail).
    cv.fill(0, 0, cw, rh, sk.strip);
    let tw = CTL;
    let tx = if rail { (cw - tw) / 2 } else { cw - EDGE - tw };
    let ty = (rh - CTL) / 2;
    if let Some(c) = wash(ch, Hot::Side(SideHit::Toggle), sk) {
        rrect(cv, tx, ty, tw, CTL, CTL_R, c, false);
    }
    glyph(cv, if rail { Glyph::Fwd } else { Glyph::Back }, tx as f32 + tw as f32 / 2.0, ty as f32 + CTL as f32 / 2.0, sk.dim);
    cv.fill(cw - 1, 0, 1, h, sk.rule);
}

/// AETHERSIDETABS2 (SR143): the open menu — a rounded card under the menu button, one row per `MENU` entry (its
/// chord dimmed at the right), the hovered row washed.
fn paint_menu(ch: &Chrome, cv: &mut Canvas, fonts: &Fonts, sk: &Skin) {
    if !ch.menu {
        return;
    }
    let (mx, my, mw, mh) = ch.menu_rect();
    rrect(cv, mx - 1, my - 1, mw + 2, mh + 2, MENU_R + 1, sk.field_line, false);
    rrect(cv, mx, my, mw, mh, MENU_R, sk.bar, false);
    for (i, (name, key)) in MENU.iter().enumerate() {
        let ry = my + MENU_PAD + i as i32 * MENU_ROW;
        if ch.hot == Hot::MenuItem(i) {
            rrect(cv, mx + MENU_PAD, ry, mw - 2 * MENU_PAD, MENU_ROW, CTL_R, sk.ctl_hover, false);
        }
        let x1 = mx + mw - 2 * MENU_PAD;
        let mut t1 = x1;
        if !key.is_empty() {
            let kw = fonts.0.borrow_mut().measure(key, FontSel::default(), FONT_SMALL);
            let kx = x1 as f32 - kw;
            label_at(cv, fonts, key, kx, ry, MENU_ROW, sk.dim, mx, x1, FONT_SMALL);
            t1 = kx as i32 - GAP;
        }
        let t0 = mx + 3 * MENU_PAD;
        label(cv, fonts, name, t0 as f32, ry, MENU_ROW, sk.ink, t0, t1);
    }
}

/// Paint the header into `cv` (the whole window): in the top layout the strip and the toolbar across the top, in the
/// side layout the column down the left and the toolbar over the page; then the open menu over everything.
fn paint_chrome(app: &App, cv: &mut Canvas, fonts: &Fonts) {
    let ch = &app.ch;
    let sk = skin();
    let g = geo(ch);
    let (x0, bt, by) = (ch.page_x(), ch.bar_top(), ch.bar_y());
    if ch.is_side() {
        paint_side(ch, cv, fonts, &sk);
    } else {
        cv.fill(0, 0, cv.w, TAB_H, sk.strip);
        // The hairline under the strip; the active tab is painted over it below, so it joins the toolbar seamlessly.
        cv.fill(0, TAB_H - 1, cv.w, 1, sk.rule);
    }
    cv.fill(x0, bt, cv.w - x0, BAR_H, sk.bar);
    let active = ch.strip.active();
    let lit = |id: u64| id == active || ch.hot == Hot::Tab(id) || ch.hot == Hot::Close(id);
    let body_h = TAB_H - TAB_TOP;
    for (i, &(id, tx, tw)) in g.tabs.iter().enumerate() {
        let on = id == active;
        if on {
            rrect(cv, tx, TAB_TOP, tw, body_h, TAB_R, sk.bar, true);
        } else if lit(id) {
            rrect(cv, tx + 2, TAB_TOP + 2, tw - 4, body_h - 5, CTL_R, sk.tab_hover, false);
        }
        // The hairline between two quiet neighbours.
        if let Some(&(next, _, _)) = g.tabs.get(i + 1) {
            if !lit(id) && !lit(next) {
                cv.fill(tx + tw, TAB_TOP + SEP_INSET, 1, body_h - 2 * SEP_INSET, sk.sep);
            }
        }
        let tab = &ch.strip.tabs()[i];
        // AETHERSIDETABS2: a grouped tab's run carries its group's colour along the strip's foot.
        if let Some(gr) = tab.group.and_then(|g| ch.strip.group(g)) {
            cv.fill(tx + 2, TAB_H - 3, tw - 4, 2, gr.color.rgb());
        }
        // The favicon slot (the favicon, else the letter tile), vertically centred with the title.
        let ty = TAB_TOP + (body_h - TILE) / 2;
        let tile_x = tx + TAB_PAD;
        tab_tile(cv, ch, fonts, &sk, tab, on, tile_x, ty, TILE);
        // The title, then the close affordance while the tab is hovered.
        let show_close = ch.hot == Hot::Tab(id) || ch.hot == Hot::Close(id);
        let t0 = tile_x + TILE + TILE_GAP;
        let t1 = if show_close { close_x(tx, tw) - GAP } else { tx + tw - TAB_PAD };
        if t1 > t0 + 4 {
            let s = fit(fonts, tab.label(), (t1 - t0) as f32);
            label(cv, fonts, &s, t0 as f32, TAB_TOP, body_h, if on { sk.ink } else { sk.dim }, t0, t1);
        }
        if show_close {
            let cx = close_x(tx, tw);
            let cy = TAB_TOP + (body_h - CLOSE) / 2;
            if ch.hot == Hot::Close(id) {
                let c = if ch.pressed == Some(Hot::Close(id)) { sk.ctl_press } else { sk.ctl_hover };
                rrect(cv, cx, cy, CLOSE, CLOSE, CLOSE / 2, c, false);
            }
            glyph(cv, Glyph::Cross, cx as f32 + CLOSE as f32 / 2.0, cy as f32 + CLOSE as f32 / 2.0, if on { sk.ink } else { sk.dim });
        }
    }
    if !ch.is_side() {
        // `+`: a new tab, a quiet glyph with a wash on hover.
        if let Some(c) = wash(ch, Hot::NewTab, &sk) {
            rrect(cv, g.plus.0, TAB_TOP, g.plus.1, body_h - 1, CTL_R, c, false);
        }
        glyph(cv, Glyph::Plus, g.plus.0 as f32 + g.plus.1 as f32 / 2.0, TAB_TOP as f32 + body_h as f32 / 2.0, sk.dim);
    }
    // The toolbar.
    control(cv, ch, &sk, g.back, Hot::Back, app.hist.can_go(-1), Glyph::Back);
    control(cv, ch, &sk, g.fwd, Hot::Fwd, app.hist.can_go(1), Glyph::Fwd);
    control(cv, ch, &sk, g.reload, Hot::Reload, true, Glyph::Reload);
    if let Some(r) = g.zout {
        control(cv, ch, &sk, r, Hot::ZoomOut, ch.zoom > zoom::LEVELS[0], Glyph::Minus);
    }
    if let Some(r) = g.zin {
        control(cv, ch, &sk, r, Hot::ZoomIn, ch.zoom < zoom::LEVELS[zoom::LEVELS.len() - 1], Glyph::Plus);
    }
    // The menu button: washed while its menu is open.
    if ch.menu {
        rrect(cv, g.menu.0, by, g.menu.1, CTL, CTL_R, sk.ctl_press, false);
    }
    control(cv, ch, &sk, g.menu, Hot::Menu, true, Glyph::Dots);
    // The bookmark star: the font's own star (a filled one in the accent when the page is bookmarked).
    if let Some(star) = g.star {
        let on = !ch.url.is_empty() && ch.bookmarks.contains(&ch.url);
        let enabled = !ch.url.is_empty();
        if enabled {
            if let Some(c) = wash(ch, Hot::Star, &sk) {
                rrect(cv, star.0, by, star.1, CTL, CTL_R, c, false);
            }
        }
        let s = if on { "\u{2605}" } else { "\u{2606}" };
        let gw = fonts.0.borrow_mut().measure(s, FontSel::default(), FONT);
        let ink = if on { sk.accent } else if enabled { sk.ink } else { sk.dim };
        label(cv, fonts, s, star.0 as f32 + (star.1 as f32 - gw) / 2.0, by, CTL, ink, star.0, star.0 + star.1);
    }
    // The address field: one rounded field with a quiet border; focused, the border turns the accent inside a soft
    // halo.
    let (fx, fw) = g.omni;
    let (ft, fh) = (by, CTL);
    let focused = ch.omni.is_some();
    if focused {
        rrect(cv, fx - RING, ft - RING, fw + 2 * RING, fh + 2 * RING, FIELD_R + RING, sk.halo, false);
    }
    rrect(cv, fx, ft, fw, fh, FIELD_R, if focused { sk.accent } else { sk.field_line }, false);
    rrect(cv, fx + 1, ft + 1, fw - 2, fh - 2, FIELD_R - 1, sk.field_bg, false);
    let mut x1 = fx + fw - FIELD_PAD;
    if ch.zoom != 100 {
        let z = alloc::format!("{}%", ch.zoom);
        let zw = fonts.0.borrow_mut().measure(&z, FontSel::default(), FONT_SMALL);
        let zx = (fx + fw - FIELD_PAD) as f32 - zw;
        label_at(cv, fonts, &z, zx, ft, fh, sk.dim, fx, fx + fw - FIELD_PAD / 2, FONT_SMALL);
        x1 = zx as i32 - GAP;
    }
    let tx0 = fx + FIELD_PAD;
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
                    cv.fill(a, ft + 4, b - a, fh - 8, sk.field_sel);
                }
            }
            label(cv, fonts, &text, ox, ft, fh, sk.field_ink, tx0, x1);
            let cx = (ox + caret) as i32;
            if cx >= tx0 && cx <= x1 {
                cv.fill(cx, ft + 5, 1, fh - 10, sk.accent);
            }
        }
        None if ch.url.is_empty() => {
            label(cv, fonts, PLACEHOLDER, tx0 as f32, ft, fh, sk.dim, tx0, x1);
        }
        None => {
            let s = fit(fonts, &ch.url, (x1 - tx0) as f32);
            label(cv, fonts, &s, tx0 as f32, ft, fh, sk.field_ink, tx0, x1);
        }
    }
    // The hairline between the header and the page.
    cv.fill(x0, ch.page_y() - 1, cv.w - x0, 1, sk.rule);
    paint_menu(ch, cv, fonts, &sk);
}

/// One frame onto the surface: the page canvas (logical px) scaled into the page's rectangle — right of the side
/// column and under the toolbar, or under the whole header (`Chrome::page_x` / `page_y`) — a straight copy at 100 %,
/// bilinear otherwise; the header painted over the rest (and the open menu over the page); then SYS_WIN_PRESENT.
pub fn blit(app: &App, cv: &Canvas, fonts: &Fonts) {
    let (pw, ph) = (app.ch.pw.min(MAX_W), app.ch.ph.min(MAX_H));
    let mut ui = Canvas::new(pw, ph);
    let (ox, oy) = (app.ch.page_x().clamp(0, pw), app.ch.page_y().clamp(0, ph));
    let (dw, dh) = (pw - ox, ph - oy);
    if dw > 0 && dh > 0 && cv.w > 0 && cv.h > 0 {
        let mut put = |x: i32, y: i32, c: u32| ui.px[((oy + y) * pw + ox + x) as usize] = c;
        if cv.w == dw && cv.h == dh {
            for y in 0..dh {
                for x in 0..dw {
                    put(x, y, cv.px[(y * cv.w + x) as usize]);
                }
            }
        } else {
            // 16.16 fixed point: the source sample centred under each window pixel.
            let sx = |x: i32| -> (i32, i32, u32) {
                let f = (((2 * x + 1) as i64 * cv.w as i64 * 65536) / (2 * dw as i64) - 32768).max(0);
                let x0 = ((f >> 16) as i32).min(cv.w - 1);
                (x0, (x0 + 1).min(cv.w - 1), ((f >> 8) & 0xFF) as u32)
            };
            let xs: Vec<(i32, i32, u32)> = (0..dw).map(sx).collect();
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
                    put(x as i32, y, out);
                }
            }
        }
    }
    paint_chrome(app, &mut ui, fonts);
    for y in 0..ph {
        let row = unsafe { app.surf.add(y as usize * crate::STRIDE) as *mut u32 };
        for x in 0..pw {
            unsafe { row.add(x as usize).write_volatile(0xFF00_0000 | ui.px[(y * pw + x) as usize]) };
        }
    }
    sys(una_abi::SYS_WIN_PRESENT, app.win as u64, 0, 0, 0);
}

/// ONEINSTANCE (rmbp-ledger B734): a second launch's argument (a URL or a file path) opens in a NEW tab, the omnibox left
/// unfocused (the page is the point); `follow` resolves it as a typed address does. `:: AETHER: open_arg n= tabs= ::`.
pub fn open_arg_tab(app: &mut App, page: &mut Page, fonts: &mut Fonts, arg: &str) {
    new_tab(app, page, fonts);
    app.ch.omni = None;
    crate::follow(app, page, fonts, arg, b"open-arg");
    app.dirty = true;
    Line::new(b":: AETHER: open_arg n=").dec(arg.len() as i64).put(b" tabs=").dec(app.ch.strip.len() as i64).put(b" ::").wire();
}
