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
//! content) is `SIDE_W` px wide or a `RAIL_W` px icon rail (its one toggle), one row per tab — the letter tile, the
//! ellipsised title, a close box on hover, the active row selected in the accent — and a `+` row last; it scrolls.
//! Ctrl+Shift+A opens the tab search (a title substring over the strip the shell last sent; picking one sends
//! `BrowserTabSelect`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::gio;
use gtk4::prelude::*;
use gtk4::{Box, Button, Orientation, ScrolledWindow};

use super::glyph::GlyphLabel;
use crate::NativeView;
use bandy::{PrefValue, PrincipiaCommand, SMessage, Synapse};

/// Principia's namespace and key for where the tabs stand (prefs_core `schema.rs`, docs/dev/PREFS-SCHEMA.md).
pub const LAYOUT_NS: &str = "aether";
pub const LAYOUT_KEY: &str = "tabs.layout";
/// The side column's width open, and collapsed to the icon rail (`aether::chrome::sidetabs::{SIDE_W, RAIL_W}`).
const SIDE_W: i32 = 240;
const RAIL_W: i32 = 48;
/// The longest title a side row shows, in characters.
const SIDE_LABEL_MAX: usize = 26;

/// The stored value means the side strip (`side`); anything else, and an unset row, is the top strip.
pub fn layout_is_side(v: &str) -> bool {
    v.trim().eq_ignore_ascii_case("side")
}

/// Whether the tabs stand on the side now: Principia's row, else the schema default (`top`).
pub fn stored_layout() -> bool {
    let Ok(store) = principia::prefs::PrefStore::load(principia::default_prefs_path()) else { return false };
    if let Some(PrefValue::Str(s)) = store.get(LAYOUT_NS, LAYOUT_KEY) {
        return layout_is_side(&s);
    }
    match store.effective(LAYOUT_NS, LAYOUT_KEY, |k| std::env::var(k).ok()) {
        Some((PrefValue::Str(s), _)) => layout_is_side(&s),
        _ => false,
    }
}

/// A Principia change to the layout row.
pub fn is_layout_change(msg: &SMessage) -> bool {
    matches!(msg, SMessage::Principia(PrincipiaCommand::PrefChanged { ns, key, .. }) if ns == LAYOUT_NS && key == LAYOUT_KEY)
}

thread_local! {
    /// The layout in force (true: side), the views that follow it, and whether its watch is installed.
    static SIDE: Cell<bool> = const { Cell::new(false) };
    static LISTENERS: RefCell<Vec<Rc<dyn Fn(bool)>>> = RefCell::new(Vec::new());
    static WATCHING: Cell<bool> = const { Cell::new(false) };
    static MONITOR: RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
    /// The strip as the shell last sent it: `(active, [(id, label, loading)])`, what the tab search lists.
    static LAST: RefCell<(u64, Vec<(u64, String, bool)>)> = const { RefCell::new((0, Vec::new())) };
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

/// The longest label a tab shows, in characters (the rest is an ellipsis).
const LABEL_MAX: usize = 24;

fn short(label: &str) -> String {
    clip(label, LABEL_MAX)
}

/// `label` cut to `max` characters, the last an ellipsis.
fn clip(label: &str, max: usize) -> String {
    if label.chars().count() <= max {
        label.to_string()
    } else {
        let mut s: String = label.chars().take(max - 1).collect();
        s.push('…');
        s
    }
}

/// The letter tile's letter: the label's first alphanumeric, upper-cased (`aether::chrome::tabs::Tab::initial`'s
/// fallback; the wire carries no URL or favicon per tab yet).
fn initial(label: &str) -> String {
    match label.chars().find(|c| c.is_alphanumeric()) {
        Some(c) => c.to_uppercase().collect(),
        None => "#".to_string(),
    }
}

fn tab_button(id: u64, label: &str, loading: bool, active: bool, synapse: &Synapse) -> gtk4::Widget {
    let row = Box::new(Orientation::Horizontal, 4);
    let select = Button::new();
    let text = if loading { format!("◌ {}", short(label)) } else { short(label) };
    select.set_child(Some(&GlyphLabel::new(&text)));
    select.set_tooltip_text(Some(label));
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
    row.upcast()
}

/// The strip: one button per tab (click selects, × closes) and a `+` that opens a new tab.
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
                Ok(SMessage::BrowserTabs { active, tabs: list }) => {
                    LAST.with(|l| *l.borrow_mut() = (active, list.clone()));
                    while let Some(child) = tabs.first_child() {
                        tabs.remove(&child);
                    }
                    for (id, label, loading) in &list {
                        tabs.append(&tab_button(*id, label, *loading, *id == active, &syn));
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
    outer.upcast()
}

// ---- AETHERSIDETABS (SR140, R118): the side column and the tab search -------------------------------------------

/// One side row: the letter tile, the title (hidden on the rail), and the close box (shown while hovered).
fn side_row(id: u64, label: &str, loading: bool, rail: bool, synapse: &Synapse) -> gtk4::Widget {
    let row = Box::new(Orientation::Horizontal, 8);
    row.set_margin_start(6);
    row.set_margin_end(4);
    row.set_margin_top(4);
    row.set_margin_bottom(4);
    let tile = gtk4::Frame::new(None);
    tile.set_size_request(22, 22);
    tile.set_valign(gtk4::Align::Center);
    tile.add_css_class("view");
    let letter = GlyphLabel::new(&if loading { "◌".to_string() } else { initial(label) });
    letter.set_halign(gtk4::Align::Center);
    letter.set_valign(gtk4::Align::Center);
    tile.set_child(Some(&letter));
    row.append(&tile);
    row.set_tooltip_text(Some(label));
    if rail {
        return row.upcast();
    }
    let title = GlyphLabel::new(&clip(label, SIDE_LABEL_MAX));
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

/// Rebuild the column's list from the strip: one row per tab, the `+` row last, the active row selected. `ids`
/// gets each row's tab id (None: the `+` row), which the activation handler reads.
fn fill_side(list: &gtk4::ListBox, ids: &RefCell<Vec<Option<u64>>>, active: u64, tabs: &[(u64, String, bool)], rail: bool, synapse: &Synapse) {
    list.remove_all();
    let mut v = Vec::with_capacity(tabs.len() + 1);
    let mut sel = None;
    for (k, (id, label, loading)) in tabs.iter().enumerate() {
        list.append(&side_row(*id, label, *loading, rail, synapse));
        v.push(Some(*id));
        if *id == active {
            sel = Some(k as i32);
        }
    }
    let plus = Box::new(Orientation::Horizontal, 8);
    plus.set_margin_start(6);
    plus.set_margin_top(4);
    plus.set_margin_bottom(4);
    plus.append(&GlyphLabel::new(if rail { "+" } else { "+   New tab" }));
    plus.set_tooltip_text(Some("New tab (Ctrl+T)"));
    list.append(&plus);
    v.push(None);
    *ids.borrow_mut() = v;
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
    column.set_size_request(SIDE_W, -1);
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

    let ids: Rc<RefCell<Vec<Option<u64>>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let (ids, syn) = (ids.clone(), synapse.clone());
        list.connect_row_activated(move |_, row| {
            let i = row.index();
            let id = if i >= 0 { ids.borrow().get(i as usize).copied() } else { None };
            match id {
                Some(Some(id)) => syn.fire(SMessage::BrowserTabSelect(id)),
                Some(None) => syn.fire(SMessage::BrowserTabNew),
                None => {}
            }
        });
    }
    let redraw: Rc<dyn Fn()> = {
        let (list, ids, rail, syn) = (list.clone(), ids.clone(), rail.clone(), synapse.clone());
        Rc::new(move || {
            let (active, tabs) = LAST.with(|l| l.borrow().clone());
            fill_side(&list, &ids, active, &tabs, rail.get(), &syn);
        })
    };
    {
        let (column, rail, redraw) = (column.clone(), rail.clone(), redraw.clone());
        toggle.connect_clicked(move |b| {
            let now = !rail.get();
            rail.set(now);
            column.set_size_request(if now { RAIL_W } else { SIDE_W }, -1);
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
                Ok(SMessage::BrowserTabs { active, tabs }) => {
                    LAST.with(|l| *l.borrow_mut() = (active, tabs));
                    redraw();
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
    body.upcast()
}

/// The tabs whose label holds `query` (case-insensitive), in strip order; every tab for an empty query.
fn search_labels(tabs: &[(u64, String, bool)], query: &str) -> Vec<(u64, String)> {
    let q = query.trim().to_lowercase();
    tabs.iter().filter(|(_, l, _)| l.to_lowercase().contains(&q)).map(|(id, l, _)| (*id, l.clone())).collect()
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

/// The search window: a query field over the matching tabs; Enter or a click picks one, Esc closes.
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
    entry.set_placeholder_text(Some("Search tabs"));
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
            let (active, tabs) = LAST.with(|l| l.borrow().clone());
            let found = search_labels(&tabs, &entry.text());
            for (id, label) in &found {
                let row = Box::new(Orientation::Horizontal, 8);
                row.set_margin_start(4);
                row.set_margin_top(3);
                row.set_margin_bottom(3);
                row.append(&GlyphLabel::new(&initial(label)));
                row.append(&GlyphLabel::new(&clip(label, 48)));
                if *id == active {
                    row.add_css_class("dim-label");
                }
                list.append(&row);
            }
            *hits.borrow_mut() = found.into_iter().map(|(id, _)| id).collect();
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
