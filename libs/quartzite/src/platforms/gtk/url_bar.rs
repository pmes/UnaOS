//! AETHERCHROME (SR84): the browser's address bar row, painted by GTK — Back, Forward, Reload/Stop,
//! the page-info popover, the address field (quartzite's own editor, QUARTZFONT SR64), the zoom
//! readout and the bookmark star — plus the bookmark bar under it.
//!
//! The decisions are not made here (R104: the face paints, the model decides): Enter sends the typed
//! text WITH the person's search template, read from Principia's `aether.search` (the schema default
//! when unset), as `SMessage::BrowserGo`; the shell's `aether::chrome::omnibox` decides URL vs search.
//! What the row shows comes back as `BrowserUrlChanged`, `BrowserNavState`, `BrowserBookmarks` and
//! `BrowserPageInfo`.

use gtk4::prelude::*;
use gtk4::{Box, Button, Label, MenuButton, Orientation, Popover};

use super::glyph::{GlyphEntry, GlyphLabel};
use bandy::{PrefValue, SMessage, Synapse};

/// The address row and the bookmark bar, with the handles the window's shortcuts need (Ctrl+L focuses
/// `entry`, Ctrl+Shift+B toggles `bookmark_bar`).
pub struct UrlBar {
    pub root: gtk4::Widget,
    pub entry: GlyphEntry,
    pub bookmark_bar: Box,
}

/// The person's search template: Principia `aether.search`, else its schema default; "" (the model's
/// default) when the store cannot be read.
pub fn search_template() -> String {
    let Ok(store) = principia::prefs::PrefStore::load(principia::default_prefs_path()) else { return String::new() };
    if let Some(PrefValue::Str(s)) = store.get("aether", "search") {
        return s;
    }
    match store.effective("aether", "search", |k| std::env::var(k).ok()) {
        Some((PrefValue::Str(s), _)) => s,
        _ => String::new(),
    }
}

fn glyph_button(label: &str, tooltip: &str) -> (Button, GlyphLabel) {
    let b = Button::new();
    let l = GlyphLabel::new(label);
    b.set_child(Some(&l));
    b.add_css_class("flat");
    b.set_tooltip_text(Some(tooltip));
    (b, l)
}

fn short(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut o: String = s.chars().take(max - 1).collect();
        o.push('…');
        o
    }
}

pub fn bootstrap_url_bar(synapse: Synapse) -> UrlBar {
    let root = Box::new(Orientation::Vertical, 0);
    let row = Box::new(Orientation::Horizontal, 4);
    row.set_margin_start(4);
    row.set_margin_end(4);
    row.set_margin_top(2);
    row.set_margin_bottom(2);

    let (back, _) = glyph_button("<", "Back (Alt+Left)");
    let (fwd, _) = glyph_button(">", "Forward (Alt+Right)");
    let (reload, reload_label) = glyph_button("C", "Reload (Ctrl+R) / Stop (Esc)");
    back.set_sensitive(false);
    fwd.set_sensitive(false);
    let syn = synapse.clone();
    back.connect_clicked(move |_| syn.fire(SMessage::BrowserNavBack));
    let syn = synapse.clone();
    fwd.connect_clicked(move |_| syn.fire(SMessage::BrowserNavForward));
    let loading = std::rc::Rc::new(std::cell::Cell::new(false));
    {
        let (syn, loading) = (synapse.clone(), loading.clone());
        reload.connect_clicked(move |_| {
            syn.fire(if loading.get() { SMessage::BrowserNavStop } else { SMessage::BrowserNavReload });
        });
    }

    // The page-info popover: the URL, whether the connection was secure, and what that means.
    let info = MenuButton::new();
    info.set_child(Some(&GlyphLabel::new("i")));
    info.add_css_class("flat");
    info.set_tooltip_text(Some("Page info"));
    let pop = Popover::new();
    let pop_box = Box::new(Orientation::Vertical, 6);
    pop_box.set_margin_start(8);
    pop_box.set_margin_end(8);
    pop_box.set_margin_top(8);
    pop_box.set_margin_bottom(8);
    let info_state = GlyphLabel::new("");
    let info_url = GlyphLabel::new("");
    let info_summary = Label::new(None);
    info_summary.set_wrap(true);
    info_summary.set_max_width_chars(48);
    info_summary.set_xalign(0.0);
    pop_box.append(&info_state);
    pop_box.append(&info_url);
    pop_box.append(&info_summary);
    pop.set_child(Some(&pop_box));
    info.set_popover(Some(&pop));
    {
        let syn = synapse.clone();
        pop.connect_show(move |_| syn.fire(SMessage::BrowserPageInfoAsk));
    }

    let entry = GlyphEntry::new("Search or enter address");
    entry.set_hexpand(true);
    {
        let syn = synapse.clone();
        entry.connect_activate(move |e| {
            syn.fire(SMessage::BrowserGo { input: e.text(), search: search_template() });
        });
    }

    let zoom_label = GlyphLabel::new("");
    zoom_label.set_visible(false);
    let (star, star_label) = glyph_button("☆", "Bookmark this page (Ctrl+D)");
    {
        let syn = synapse.clone();
        star.connect_clicked(move |_| syn.fire(SMessage::BrowserBookmarkToggle));
    }

    row.append(&back);
    row.append(&fwd);
    row.append(&reload);
    row.append(&info);
    row.append(&entry);
    row.append(&zoom_label);
    row.append(&star);

    // Esc in the address field (which the field itself does not take) stops the load.
    {
        let key = gtk4::EventControllerKey::new();
        let syn = synapse.clone();
        key.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gtk4::gdk::Key::Escape {
                syn.fire(SMessage::BrowserNavStop);
                return gtk4::glib::Propagation::Stop;
            }
            gtk4::glib::Propagation::Proceed
        });
        row.add_controller(key);
    }

    let bookmark_bar = Box::new(Orientation::Horizontal, 2);
    bookmark_bar.set_margin_start(4);
    bookmark_bar.set_margin_end(4);
    bookmark_bar.set_visible(false);

    root.append(&row);
    root.append(&bookmark_bar);

    let mut rx = synapse.subscribe();
    let (syn, entry_c, bar) = (synapse.clone(), entry.clone(), bookmark_bar.clone());
    gtk4::glib::spawn_future_local(async move {
        loop {
            match rx.recv().await {
                // The address field mirrors the active tab's URL — unless the person is typing in it.
                Ok(SMessage::BrowserUrlChanged(url)) => {
                    if !entry_c.has_focus() {
                        entry_c.set_text(&url);
                    }
                }
                Ok(SMessage::BrowserNavState { can_back, can_forward, loading: busy, zoom_pct }) => {
                    back.set_sensitive(can_back);
                    fwd.set_sensitive(can_forward);
                    loading.set(busy);
                    reload_label.set_text(if busy { "X" } else { "C" });
                    zoom_label.set_text(&format!("{zoom_pct}%"));
                    zoom_label.set_visible(zoom_pct != 100);
                }
                Ok(SMessage::BrowserBookmarks { items, current }) => {
                    star_label.set_text(if current { "★" } else { "☆" });
                    while let Some(child) = bar.first_child() {
                        bar.remove(&child);
                    }
                    for (title, url) in items {
                        let name = if title.trim().is_empty() { url.clone() } else { title };
                        let (b, _) = glyph_button(&short(&name, 20), &url);
                        let syn = syn.clone();
                        b.connect_clicked(move |_| syn.fire(SMessage::OpenDocument { url: url.clone() }));
                        bar.append(&b);
                    }
                }
                Ok(SMessage::BrowserPageInfo { url, secure, summary }) => {
                    info_state.set_text(if secure { "Secure connection" } else { "Not secure" });
                    info_url.set_text(&short(&url, 64));
                    info_summary.set_text(&summary);
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });

    UrlBar { root: root.upcast(), entry, bookmark_bar }
}
