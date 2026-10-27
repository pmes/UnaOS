//! AETHERCHROME (SR84): the browser's tab strip, painted by GTK. The strip's MODEL is
//! `aether::chrome::tabs` (in the shell); this face only draws what `SMessage::BrowserTabs` says and
//! sends the person's clicks back as `BrowserTabSelect` / `BrowserTabClose` / `BrowserTabNew`.
//! Labels are quartzite's own text (QUARTZFONT, SR64).
//!
//! AETHERSIDETABS (SR140, R118): the strip can also stand on the LEFT, vertically — Chrome's side tabs, for the
//! many-tabs user. Where it stands is Principia's `aether.tabs.layout` (`top` | `side`, default `top`; the model's
//! `aether::chrome::sidetabs::TabLayout`), read when the face is built and again on a `PrefChanged` for it on this
//! bus or a change to the store's folder (the `a11y_prefs` lanes). A switch only shows one strip and hides the
//! other: no tab, engine or page is touched. The side column ([`bootstrap_side_tabs`], a `GtkListBox` beside the
//! content) is `chrome_rows::SIDE_W` px wide or a `chrome_rows::RAIL_W` px icon rail (its one toggle), one row per
//! tab — the letter tile, the ellipsised title, a close box on hover, the active row selected in the accent — and a
//! `+` row last; it scrolls. ONETEXT (SR210): the rows, their verbs, the clip and the widths are
//! `widgets/chrome_rows.rs`'s, the same ones the AppKit face reads.
//! Ctrl+Shift+A opens the tab search (a title substring over the strip the shell last sent; picking one sends
//! `BrowserTabSelect`).
//!
//! AETHERSIDETABS2 (SR143): the wire carries each tab's URL, group and letter tile, the groups (name, colour,
//! collapsed), and each tab's favicon tile (`BrowserTabFavicon`, 16 px). Both strips show the favicon in place of
//! the letter tile, and the groups — the side column a header per group (colour dot, name, count, chevron; a click
//! collapses or expands it) with a coloured stripe on its tabs, the top strip a chip before each run and a coloured
//! underline under it. Every tab DRAGS: dropped on a tab it takes that tab's place (`BrowserTabMove`; between two
//! members of a group it joins the group, the model's rule), on a group's header or chip it joins the group
//! (`BrowserTabGroupAdd`), on `+` it goes to the end. A right-click opens the group verbs (a new group with the
//! tab, into another group, out of its own, close; on a group: collapse, name, next colour, ungroup). The search
//! matches the title or the URL.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{gdk, gio, glib};
use gtk4::{Box, Button, Orientation, ScrolledWindow};

use super::glyph::GlyphLabel;
use crate::widgets::chrome_rows::{self, clip, DropOn, MenuRow, SideRow, Tile, SIDE_LABEL_MAX};
use crate::NativeView;
use bandy::{BrowserTabGroupRow, BrowserTabRow, SMessage, Synapse};

/// STRIP2 (SR219): the layout row (Principia's `aether.tabs.layout`) is chrome_rows', the one read both faces make;
/// the names stay public here.
pub use crate::widgets::chrome_rows::{is_layout_change, layout_is_side, stored_layout, LAYOUT_KEY, LAYOUT_NS};

thread_local! {
    /// The layout in force (true: side), the views that follow it, and whether its watch is installed.
    static SIDE: Cell<bool> = const { Cell::new(false) };
    static LISTENERS: RefCell<Vec<Rc<dyn Fn(bool)>>> = RefCell::new(Vec::new());
    static WATCHING: Cell<bool> = const { Cell::new(false) };
    static MONITOR: RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
    /// The tab search window, built on first use.
    static SEARCH: RefCell<Option<gtk4::Window>> = const { RefCell::new(None) };
}

/// Re-read the row; tell every view when it changed.
fn relayout() {
    let side = stored_layout();
    if SIDE.with(|c| c.replace(side)) != side {
        let ls: Vec<Rc<dyn Fn(bool)>> = LISTENERS.with(|l| l.borrow().clone());
        for l in ls {
            l(side);
        }
    }
}

/// Read the row now and watch it (once per process): the bus lane and the store-folder lane.
fn watch_layout(synapse: &Synapse) {
    if WATCHING.with(|w| w.replace(true)) {
        return;
    }
    SIDE.with(|c| c.set(stored_layout()));
    let mut rx = synapse.subscribe();
    gtk4::glib::spawn_future_local(async move {
        loop {
            match rx.recv().await {
                Ok(m) if is_layout_change(&m) => relayout(),
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
    let dir = principia::default_prefs_path();
    match gio::File::for_path(&dir).monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) {
        Ok(monitor) => {
            monitor.connect_changed(|_, _, _, _| relayout());
            MONITOR.with(|m| *m.borrow_mut() = Some(monitor));
        }
        Err(e) => log::warn!("[aether] tab layout: no monitor on {} ({e}); changes arrive on the bus only", dir.display()),
    }
}

/// Run `f` with the layout now and on every change.
fn on_layout(synapse: &Synapse, f: impl Fn(bool) + 'static) {
    watch_layout(synapse);
    let f: Rc<dyn Fn(bool)> = Rc::new(f);
    f(SIDE.with(|c| c.get()));
    LISTENERS.with(|l| l.borrow_mut().push(f));
}

/// A top-strip label, cut at chrome_rows' `TOP_LABEL_MAX` characters (the rest is an ellipsis).
fn short(label: &str) -> String {
    clip(label, chrome_rows::TOP_LABEL_MAX)
}

// ---- AETHERSIDETABS2 (SR143): the strip as the shell sends it, the favicons, the drag ----------------------------

/// The strip as the shell last sent it (`SMessage::BrowserTabs`): what both strips draw and the tab search lists.
/// STRIP2 (SR219): chrome_rows' `TabWire` (its `group`, `pos`, `drop_index`, `drop_verb`, `search`).
use crate::widgets::chrome_rows::TabWire as Strip;

thread_local! {
    static LAST: RefCell<Strip> = RefCell::new(Strip::default());
    /// Each tab's favicon tile (`SMessage::BrowserTabFavicon`), by tab id; dropped when the tab is gone.
    static ICONS: RefCell<HashMap<u64, gdk::Texture>> = RefCell::new(HashMap::new());
}

/// Take a strip-shaped message into the shared state; true when the strips should redraw.
fn take(m: &SMessage) -> bool {
    match m {
        SMessage::BrowserTabs { active, tabs, groups } => {
            ICONS.with(|i| i.borrow_mut().retain(|id, _| tabs.iter().any(|t| t.id == *id)));
            LAST.with(|l| *l.borrow_mut() = Strip { active: *active, tabs: tabs.clone(), groups: groups.clone() });
            true
        }
        SMessage::BrowserTabFavicon { id, size, rgba } => {
            match chrome_rows::favicon_rgba(*size, rgba) {
                None => {
                    ICONS.with(|i| i.borrow_mut().remove(id));
                }
                Some(px) => {
                    let n = *size as usize;
                    let bytes = glib::Bytes::from(px);
                    let tex = gdk::MemoryTexture::new(n as i32, n as i32, gdk::MemoryFormat::R8g8b8a8, &bytes, n * 4);
                    ICONS.with(|i| i.borrow_mut().insert(*id, tex.upcast()));
                }
            }
            true
        }
        _ => false,
    }
}

/// `0xRRGGBB` as cairo's three channels.
fn rgb3(rgb: u32) -> (f64, f64, f64) {
    (((rgb >> 16) & 0xFF) as f64 / 255.0, ((rgb >> 8) & 0xFF) as f64 / 255.0, (rgb & 0xFF) as f64 / 255.0)
}

/// A group's colour as a dot `d` px across (the side header, the top chip).
fn dot(rgb: u32, d: i32) -> gtk4::DrawingArea {
    let a = gtk4::DrawingArea::new();
    a.set_content_width(d);
    a.set_content_height(d);
    a.set_valign(gtk4::Align::Center);
    a.set_draw_func(move |_, cr, w, h| {
        let (r, g, b) = rgb3(rgb);
        cr.set_source_rgb(r, g, b);
        let rad = w.min(h) as f64 / 2.0;
        cr.arc(w as f64 / 2.0, h as f64 / 2.0, rad, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
    });
    a
}

/// A group's colour as a bar (`w` x `h`; -1 stretches): the top strip's underline, the side row's stripe.
fn bar(rgb: u32, w: i32, h: i32) -> gtk4::DrawingArea {
    let a = gtk4::DrawingArea::new();
    if w > 0 {
        a.set_content_width(w);
    } else {
        a.set_hexpand(true);
    }
    if h > 0 {
        a.set_content_height(h);
    } else {
        a.set_vexpand(true);
    }
    a.set_draw_func(move |_, cr, w, h| {
        let (r, g, b) = rgb3(rgb);
        cr.set_source_rgb(r, g, b);
        cr.rectangle(0.0, 0.0, w as f64, h as f64);
        let _ = cr.fill();
    });
    a
}

/// The favicon slot: the tab's tile when the engine sent one, else the letter tile (`◌` while loading) — STRIP2
/// (SR219): chrome_rows' `tile_face` rule.
fn tile(t: &BrowserTabRow, d: i32) -> gtk4::Widget {
    let tex = ICONS.with(|i| i.borrow().get(&t.id).cloned());
    let letter = match chrome_rows::tile_face(t, tex.is_some()) {
        Tile::Icon => {
            let img = gtk4::Image::from_paintable(tex.as_ref());
            img.set_pixel_size(chrome_rows::FAVICON_PX as i32);
            img.set_size_request(d, d);
            img.set_valign(gtk4::Align::Center);
            return img.upcast();
        }
        Tile::Letter(l) => l,
    };
    let frame = gtk4::Frame::new(None);
    frame.set_size_request(d, d);
    frame.set_valign(gtk4::Align::Center);
    frame.add_css_class("view");
    let letter = GlyphLabel::new(&letter);
    letter.set_halign(gtk4::Align::Center);
    letter.set_valign(gtk4::Align::Center);
    frame.set_child(Some(&letter));
    frame.upcast()
}

/// `w` can be dragged: it carries tab `id` (chrome_rows' `drag_payload`).
fn drag_source(w: &impl IsA<gtk4::Widget>, id: u64) {
    let src = gtk4::DragSource::new();
    src.set_actions(gdk::DragAction::MOVE);
    src.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&chrome_rows::drag_payload(id).to_value())));
    w.as_ref().add_controller(src);
}

/// `w` takes a dropped tab: `on_drop(dragged id)`.
fn drop_target(w: &impl IsA<gtk4::Widget>, on_drop: impl Fn(u64) + 'static) {
    let t = gtk4::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
    t.connect_drop(move |_, v, _, _| {
        let id = v.get::<String>().ok().and_then(|s| chrome_rows::drag_id(&s));
        match id {
            Some(id) => {
                on_drop(id);
                true
            }
            None => false,
        }
    });
    w.as_ref().add_controller(t);
}

/// A tab dropped on `on` (a tab, a group's header or chip, `+`): chrome_rows' `drop_verb` over the strip.
fn drop_on(synapse: &Synapse, on: DropOn) -> impl Fn(u64) + 'static {
    let syn = synapse.clone();
    move |id| {
        if let Some(m) = LAST.with(|l| l.borrow().drop_verb(id, on)) {
            syn.fire(m);
        }
    }
}

/// A tab dropped on tab `onto`: the move (the model joins or leaves groups by where it lands).
fn drop_on_tab(synapse: &Synapse, onto: u64) -> impl Fn(u64) + 'static {
    drop_on(synapse, DropOn::Tab(onto))
}

/// A tab dropped on group `group`'s header or chip: it joins the group.
fn drop_on_group(synapse: &Synapse, group: u64) -> impl Fn(u64) + 'static {
    drop_on(synapse, DropOn::Group(group))
}

/// The right-click menu on `w`: `items` are (label, the message it sends); a `None` message is the rename entry
/// for that group.
fn context_menu(w: &impl IsA<gtk4::Widget>, synapse: &Synapse, items: impl Fn() -> Vec<MenuRow> + 'static) {
    let click = gtk4::GestureClick::new();
    click.set_button(3);
    let (anchor, syn) = (w.as_ref().clone(), synapse.clone());
    click.connect_pressed(move |g, _, _, _| {
        g.set_state(gtk4::EventSequenceState::Claimed);
        let pop = gtk4::Popover::new();
        let col = Box::new(Orientation::Vertical, 2);
        for (label, msg, gid) in items() {
            match msg {
                Some(m) => {
                    let b = Button::new();
                    b.set_child(Some(&GlyphLabel::new(&label)));
                    b.add_css_class("flat");
                    let (syn, pop) = (syn.clone(), pop.clone());
                    b.connect_clicked(move |_| {
                        syn.fire(m.clone());
                        pop.popdown();
                    });
                    col.append(&b);
                }
                None => {
                    let e = gtk4::Entry::new();
                    e.set_placeholder_text(Some(&label));
                    let (syn, pop) = (syn.clone(), pop.clone());
                    e.connect_activate(move |e| {
                        syn.fire(SMessage::BrowserTabGroupRename { group: gid, name: e.text().to_string() });
                        pop.popdown();
                    });
                    col.append(&e);
                }
            }
        }
        pop.set_child(Some(&col));
        pop.set_parent(&anchor);
        pop.connect_closed(|p| p.unparent());
        pop.popup();
    });
    w.as_ref().add_controller(click);
}

/// A tab's menu over the strip now (chrome_rows' `tab_menu`).
fn tab_menu(id: u64) -> Vec<MenuRow> {
    LAST.with(|l| chrome_rows::tab_menu(&l.borrow(), id))
}

/// A group's menu over the strip now (chrome_rows' `group_menu`).
fn group_menu(gid: u64) -> Vec<MenuRow> {
    LAST.with(|l| chrome_rows::group_menu(&l.borrow(), gid))
}

// ---- the top strip ---------------------------------------------------------------------------------------------

fn tab_button(t: &BrowserTabRow, active: bool, group: Option<&BrowserTabGroupRow>, synapse: &Synapse) -> gtk4::Widget {
    let id = t.id;
    let row = Box::new(Orientation::Horizontal, 4);
    let select = Button::new();
    let inner = Box::new(Orientation::Horizontal, 6);
    inner.append(&tile(t, 18));
    inner.append(&GlyphLabel::new(&short(&t.label)));
    select.set_child(Some(&inner));
    select.set_tooltip_text(Some(if t.url.is_empty() { &t.label } else { &t.url }));
    if !active {
        select.add_css_class("flat");
    }
    let syn = synapse.clone();
    select.connect_clicked(move |_| syn.fire(SMessage::BrowserTabSelect(id)));
    let close = Button::new();
    close.set_child(Some(&GlyphLabel::new("×")));
    close.add_css_class("flat");
    close.set_tooltip_text(Some("Close tab (Ctrl+W)"));
    let syn = synapse.clone();
    close.connect_clicked(move |_| syn.fire(SMessage::BrowserTabClose(Some(id))));
    row.append(&select);
    row.append(&close);
    if active {
        row.add_css_class("linked");
    }
    // AETHERSIDETABS2 (SR143): a grouped tab stands over its group's coloured underline; every tab drags.
    let cell = Box::new(Orientation::Vertical, 1);
    cell.append(&row);
    if let Some(g) = group {
        cell.append(&bar(g.rgb, -1, 3));
    }
    drag_source(&select, id);
    drop_target(&cell, drop_on_tab(synapse, id));
    context_menu(&cell, synapse, move || tab_menu(id));
    cell.upcast()
}

/// A group's chip in the top strip: its dot and name (its colour's name when unnamed); a click collapses or
/// expands it, a tab dropped on it joins it.
fn group_chip(g: &BrowserTabGroupRow, synapse: &Synapse) -> gtk4::Widget {
    let gid = g.id;
    let b = Button::new();
    let inner = Box::new(Orientation::Horizontal, 6);
    inner.append(&dot(g.rgb, 10));
    let name = if g.name.is_empty() { String::new() } else { short(&g.name) };
    if !name.is_empty() {
        inner.append(&GlyphLabel::new(&name));
    }
    b.set_child(Some(&inner));
    b.add_css_class("flat");
    b.set_tooltip_text(Some(if g.collapsed { "Expand the group" } else { "Collapse the group" }));
    let syn = synapse.clone();
    b.connect_clicked(move |_| syn.fire(SMessage::BrowserTabGroupToggle(gid)));
    drop_target(&b, drop_on_group(synapse, gid));
    context_menu(&b, synapse, move || group_menu(gid));
    b.upcast()
}

/// Rebuild the top strip: each group's chip before its run (the run hidden while collapsed), then its tabs.
/// STRIP2 (SR219): the walk is chrome_rows' `top_rows`; this face only builds each cell.
fn fill_top(tabs: &Box, synapse: &Synapse) {
    while let Some(child) = tabs.first_child() {
        tabs.remove(&child);
    }
    let s = LAST.with(|l| l.borrow().clone());
    for r in chrome_rows::top_rows(s.active, &s.tabs, &s.groups) {
        match r {
            SideRow::Group { id, .. } => {
                if let Some(g) = s.group(id) {
                    tabs.append(&group_chip(g, synapse));
                }
            }
            SideRow::Tab { id, active, .. } => {
                let Some(t) = s.tabs.iter().find(|t| t.id == id) else { continue };
                tabs.append(&tab_button(t, active, t.group.and_then(|g| s.group(g)), synapse));
            }
            SideRow::New => {}
        }
    }
}

/// The strip: one button per tab (click selects, × closes, drag reorders, right-click groups), each group's chip,
/// and a `+` that opens a new tab.
pub fn bootstrap_tab_strip(synapse: Synapse) -> NativeView {
    let outer = Box::new(Orientation::Horizontal, 4);
    outer.set_margin_start(4);
    outer.set_margin_end(4);
    outer.set_margin_top(2);
    let tabs = Box::new(Orientation::Horizontal, 2);
    let scroller = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Automatic)
        .vscrollbar_policy(gtk4::PolicyType::Never)
        .hexpand(true)
        .child(&tabs)
        .build();
    let new_tab = Button::new();
    new_tab.set_child(Some(&GlyphLabel::new("+")));
    new_tab.add_css_class("flat");
    new_tab.set_tooltip_text(Some("New tab (Ctrl+T)"));
    let syn = synapse.clone();
    new_tab.connect_clicked(move |_| syn.fire(SMessage::BrowserTabNew));
    {
        // A tab dropped on `+` goes to the end of the strip.
        drop_target(&new_tab, drop_on(&synapse, DropOn::End));
    }
    outer.append(&scroller);
    outer.append(&new_tab);
    // AETHERSIDETABS (SR140): the top strip shows only in the `top` layout; Ctrl+Shift+A searches the tabs.
    {
        let o = outer.clone();
        on_layout(&synapse, move |side| o.set_visible(!side));
    }
    install_tab_search(&outer, &synapse);

    let mut rx = synapse.subscribe();
    let syn = synapse.clone();
    gtk4::glib::spawn_future_local(async move {
        loop {
            match rx.recv().await {
                Ok(m) => {
                    if take(&m) {
                        fill_top(&tabs, &syn);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
    outer.upcast()
}

// ---- AETHERSIDETABS (SR140, R118): the side column and the tab search -------------------------------------------

/// One side row: the group's stripe, the favicon or the letter tile, the title (hidden on the rail), and the close
/// box (shown while hovered).
fn side_row(t: &BrowserTabRow, group: Option<&BrowserTabGroupRow>, rail: bool, synapse: &Synapse) -> gtk4::Widget {
    let id = t.id;
    let row = Box::new(Orientation::Horizontal, 8);
    row.set_margin_start(if group.is_some() { 2 } else { 6 });
    row.set_margin_end(4);
    row.set_margin_top(4);
    row.set_margin_bottom(4);
    if let Some(g) = group {
        row.append(&bar(g.rgb, 3, -1));
    }
    row.append(&tile(t, 22));
    row.set_tooltip_text(Some(if t.url.is_empty() { &t.label } else { &t.url }));
    drag_source(&row, id);
    drop_target(&row, drop_on_tab(synapse, id));
    context_menu(&row, synapse, move || tab_menu(id));
    if rail {
        return row.upcast();
    }
    let title = GlyphLabel::new(&clip(&t.label, SIDE_LABEL_MAX));
    title.set_hexpand(true);
    title.set_halign(gtk4::Align::Start);
    title.set_valign(gtk4::Align::Center);
    row.append(&title);
    let close = Button::new();
    close.set_child(Some(&GlyphLabel::new("×")));
    close.add_css_class("flat");
    close.set_tooltip_text(Some("Close tab (Ctrl+W)"));
    close.set_opacity(0.0);
    let syn = synapse.clone();
    close.connect_clicked(move |_| syn.fire(SMessage::BrowserTabClose(Some(id))));
    row.append(&close);
    let hover = gtk4::EventControllerMotion::new();
    {
        let c = close.clone();
        hover.connect_enter(move |_, _, _| c.set_opacity(1.0));
    }
    {
        let c = close.clone();
        hover.connect_leave(move |_| c.set_opacity(0.0));
    }
    row.add_controller(hover);
    row.upcast()
}

/// A group's header row: its colour dot, name, tab count and disclosure chevron (the dot alone on the rail).
/// Activating it collapses or expands the group; a tab dropped on it joins the group.
fn group_row(g: &BrowserTabGroupRow, count: usize, rail: bool, synapse: &Synapse) -> gtk4::Widget {
    let gid = g.id;
    let row = Box::new(Orientation::Horizontal, 8);
    row.set_margin_start(8);
    row.set_margin_end(8);
    row.set_margin_top(4);
    row.set_margin_bottom(2);
    row.append(&dot(g.rgb, 10));
    row.set_tooltip_text(Some(&format!("{} — {} tabs", if g.name.is_empty() { &g.color } else { &g.name }, count)));
    drop_target(&row, drop_on_group(synapse, gid));
    context_menu(&row, synapse, move || group_menu(gid));
    if rail {
        row.set_halign(gtk4::Align::Center);
        return row.upcast();
    }
    let name = GlyphLabel::new(&clip(if g.name.is_empty() { &g.color } else { &g.name }, SIDE_LABEL_MAX - 6));
    name.set_hexpand(true);
    name.set_halign(gtk4::Align::Start);
    row.append(&name);
    let n = GlyphLabel::new(&count.to_string());
    n.add_css_class("dim-label");
    row.append(&n);
    row.append(&GlyphLabel::new(if g.collapsed { "▸" } else { "▾" }));
    row.upcast()
}

/// Rebuild the column's list from the strip: the model's rows (a group's header, its tabs unless collapsed, the
/// ungrouped tabs), the `+` row last, the active row selected. `ids` gets what each row stands for, which the
/// activation handler reads.
fn fill_side(list: &gtk4::ListBox, ids: &RefCell<Vec<SideRow>>, rail: bool, synapse: &Synapse) {
    let s = LAST.with(|l| l.borrow().clone());
    list.remove_all();
    // ONETEXT (SR210): the walk is chrome_rows' (`TabStrip::rows` over the wire); this face only builds each row.
    let rows = chrome_rows::side_rows(s.active, &s.tabs, &s.groups);
    let mut sel = None;
    for (k, r) in rows.iter().enumerate() {
        match r {
            SideRow::Group { id, count, .. } => {
                if let Some(g) = s.group(*id) {
                    list.append(&group_row(g, *count, rail, synapse));
                }
            }
            SideRow::Tab { id, active, .. } => {
                let Some(t) = s.tabs.iter().find(|t| t.id == *id) else { continue };
                if *active {
                    sel = Some(k as i32);
                }
                list.append(&side_row(t, t.group.and_then(|g| s.group(g)), rail, synapse));
            }
            SideRow::New => {
                let plus = Box::new(Orientation::Horizontal, 8);
                plus.set_margin_start(6);
                plus.set_margin_top(4);
                plus.set_margin_bottom(4);
                plus.append(&GlyphLabel::new(if rail { "+" } else { "+   New tab" }));
                plus.set_tooltip_text(Some("New tab (Ctrl+T)"));
                drop_target(&plus, drop_on(synapse, DropOn::End));
                list.append(&plus);
            }
        }
    }
    *ids.borrow_mut() = rows;
    match sel.and_then(|k| list.row_at_index(k)) {
        Some(row) => {
            list.select_row(Some(&row));
        }
        None => list.unselect_all(),
    }
}

/// The side column beside `content`: a horizontal box `[column | content]` that the window puts where the content
/// stood. The column shows only in the `side` layout (live; the content stays put either way).
pub fn bootstrap_side_tabs(synapse: Synapse, content: &impl IsA<gtk4::Widget>) -> NativeView {
    let body = Box::new(Orientation::Horizontal, 0);
    let column = Box::new(Orientation::Vertical, 0);
    column.set_size_request(chrome_rows::SIDE_W as i32, -1);
    column.add_css_class("sidebar");
    let rail = Rc::new(Cell::new(false));
    let toggle = Button::new();
    toggle.set_child(Some(&GlyphLabel::new("«")));
    toggle.add_css_class("flat");
    toggle.set_halign(gtk4::Align::End);
    toggle.set_tooltip_text(Some("Collapse the tabs to icons"));
    let list = gtk4::ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::Single);
    list.add_css_class("navigation-sidebar");
    let scroller = ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Automatic)
        .vexpand(true)
        .child(&list)
        .build();
    column.append(&toggle);
    column.append(&scroller);
    body.append(&column);
    body.append(&gtk4::Separator::new(Orientation::Vertical));
    content.as_ref().set_hexpand(true);
    body.append(content);

    let ids: Rc<RefCell<Vec<SideRow>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let (ids, syn) = (ids.clone(), synapse.clone());
        list.connect_row_activated(move |_, row| {
            let i = row.index();
            let verb = if i >= 0 { ids.borrow().get(i as usize).map(|r| chrome_rows::row_verb(r, false)) } else { None };
            if let Some(m) = verb {
                syn.fire(m);
            }
        });
    }
    let redraw: Rc<dyn Fn()> = {
        let (list, ids, rail, syn) = (list.clone(), ids.clone(), rail.clone(), synapse.clone());
        Rc::new(move || fill_side(&list, &ids, rail.get(), &syn))
    };
    {
        let (column, rail, redraw) = (column.clone(), rail.clone(), redraw.clone());
        toggle.connect_clicked(move |b| {
            let now = !rail.get();
            rail.set(now);
            column.set_size_request((if now { chrome_rows::RAIL_W } else { chrome_rows::SIDE_W }) as i32, -1);
            b.set_child(Some(&GlyphLabel::new(if now { "»" } else { "«" })));
            b.set_halign(if now { gtk4::Align::Center } else { gtk4::Align::End });
            b.set_tooltip_text(Some(if now { "Expand the tabs" } else { "Collapse the tabs to icons" }));
            redraw();
        });
    }
    {
        let c = column.clone();
        on_layout(&synapse, move |side| c.set_visible(side));
    }
    install_tab_search(&column, &synapse);
    redraw();

    let mut rx = synapse.subscribe();
    gtk4::glib::spawn_future_local(async move {
        loop {
            match rx.recv().await {
                Ok(m) => {
                    if take(&m) {
                        redraw();
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
    body.upcast()
}

/// Ctrl+Shift+A on the window (a global-scope shortcut on `anchor`) opens the tab search.
fn install_tab_search(anchor: &impl IsA<gtk4::Widget>, synapse: &Synapse) {
    use gtk4::{CallbackAction, Shortcut, ShortcutController, ShortcutScope, ShortcutTrigger};
    let ctl = ShortcutController::new();
    ctl.set_scope(ShortcutScope::Global);
    let (w, syn) = (anchor.as_ref().clone(), synapse.clone());
    let open = CallbackAction::new(move |_, _| {
        open_tab_search(&w, &syn);
        gtk4::glib::Propagation::Stop
    });
    for t in ["<Control><Shift>a", "<Control><Shift>A"] {
        if let Some(trigger) = ShortcutTrigger::parse_string(t) {
            ctl.add_shortcut(Shortcut::new(Some(trigger), Some(open.clone())));
        }
    }
    anchor.as_ref().add_controller(ctl);
}

/// The search window: a query field over the matching tabs (title or URL); Enter or a click picks one, Esc closes.
fn open_tab_search(anchor: &gtk4::Widget, synapse: &Synapse) {
    if let Some(win) = SEARCH.with(|s| s.borrow().clone()) {
        win.present();
        return;
    }
    let win = gtk4::Window::builder().title("Search tabs").modal(true).default_width(440).default_height(360).hide_on_close(true).build();
    if let Some(parent) = anchor.root().and_then(|r| r.downcast::<gtk4::Window>().ok()) {
        win.set_transient_for(Some(&parent));
    }
    let col = Box::new(Orientation::Vertical, 6);
    col.set_margin_start(8);
    col.set_margin_end(8);
    col.set_margin_top(8);
    col.set_margin_bottom(8);
    let entry = gtk4::SearchEntry::new();
    entry.set_placeholder_text(Some("Search tabs by title or address"));
    let list = gtk4::ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::Single);
    let scroller = ScrolledWindow::builder().vexpand(true).hscrollbar_policy(gtk4::PolicyType::Never).child(&list).build();
    col.append(&entry);
    col.append(&scroller);
    win.set_child(Some(&col));

    let hits: Rc<RefCell<Vec<u64>>> = Rc::new(RefCell::new(Vec::new()));
    let refill: Rc<dyn Fn()> = {
        let (list, entry, hits) = (list.clone(), entry.clone(), hits.clone());
        Rc::new(move || {
            list.remove_all();
            let s = LAST.with(|l| l.borrow().clone());
            let found = s.search(&entry.text());
            for t in &found {
                let row = Box::new(Orientation::Horizontal, 8);
                row.set_margin_start(4);
                row.set_margin_top(3);
                row.set_margin_bottom(3);
                row.append(&tile(t, 18));
                let text = Box::new(Orientation::Vertical, 0);
                text.append(&GlyphLabel::new(&clip(&t.label, 48)));
                if !t.url.is_empty() {
                    let u = GlyphLabel::new(&clip(&t.url, 56));
                    u.add_css_class("dim-label");
                    text.append(&u);
                }
                row.append(&text);
                if t.id == s.active {
                    row.add_css_class("dim-label");
                }
                list.append(&row);
            }
            *hits.borrow_mut() = found.into_iter().map(|t| t.id).collect();
            if let Some(r) = list.row_at_index(0) {
                list.select_row(Some(&r));
            }
        })
    };
    let pick: Rc<dyn Fn(i32)> = {
        let (hits, syn, win) = (hits.clone(), synapse.clone(), win.clone());
        Rc::new(move |i| {
            if let Some(id) = (i >= 0).then(|| hits.borrow().get(i as usize).copied()).flatten() {
                syn.fire(SMessage::BrowserTabSelect(id));
            }
            win.set_visible(false);
        })
    };
    {
        let refill = refill.clone();
        entry.connect_search_changed(move |_| refill());
    }
    {
        let (pick, list) = (pick.clone(), list.clone());
        entry.connect_activate(move |_| pick(list.selected_row().map(|r| r.index()).unwrap_or(0)));
    }
    {
        let win = win.clone();
        entry.connect_stop_search(move |_| win.set_visible(false));
    }
    {
        let pick = pick.clone();
        list.connect_row_activated(move |_, row| pick(row.index()));
    }
    {
        // Up / Down move the highlight while the field keeps the keys.
        let keys = gtk4::EventControllerKey::new();
        let list = list.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            let step = match key {
                gtk4::gdk::Key::Down => 1,
                gtk4::gdk::Key::Up => -1,
                _ => return gtk4::glib::Propagation::Proceed,
            };
            let cur = list.selected_row().map(|r| r.index()).unwrap_or(-1);
            if let Some(r) = list.row_at_index((cur + step).max(0)) {
                list.select_row(Some(&r));
            }
            gtk4::glib::Propagation::Stop
        });
        entry.add_controller(keys);
    }
    {
        // Every time it opens: a fresh query over the strip as it is now.
        let (entry, refill) = (entry.clone(), refill.clone());
        win.connect_show(move |_| {
            entry.set_text("");
            refill();
            entry.grab_focus();
        });
    }
    SEARCH.with(|s| *s.borrow_mut() = Some(win.clone()));
    win.present();
}
