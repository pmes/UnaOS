//! AETHERDEVTOOLS2 (LEDGER SR112): the developer-tools pane's GTK face — the pane beside (or under) the
//! page, the splitter between them, and the dock side as a Principia preference.
//!
//! R104: the face paints, the model decides. The model is `aether::chrome::DevtoolsChrome` (open, dock,
//! share; `split` gives the page and pane rects), owned by the shell's coordinator; quartzite does not
//! link the engine, so everything crosses bandy:
//!
//! - the content area's size goes out as `BrowserResize` (an invisible sizer under the split, so the
//!   size is the WHOLE area, page plus pane — the shell splits it and the engine's device size follows
//!   the page rect);
//! - `BrowserDevtoolsLayout` comes back with `split`'s rects: the pane shows or hides, the `Paned` turns
//!   horizontal (right dock) or vertical (bottom), and its position is set to the page rect's extent;
//! - dragging the splitter sends the pane's share as `BrowserDevtoolsShare` (the model clamps it);
//! - the panel tabs send `BrowserDevtoolsPanel`, the Console field `BrowserDevtoolsEval`, the dock
//!   button `BrowserDevtoolsDock` (and stores Principia `aether.devtools.dock`), the close button the
//!   toggle key;
//! - `BrowserDevtoolsLines` (the engine's `DevTools::render`) fills the text view.
//!
//! The F12 / Ctrl+Shift+I hook is the window's capture-phase shortcut controller (`super::mod.rs`), so
//! the key is taken before the page sees it and handed to `DevtoolsChrome::on_key` in the shell.

use gtk4::prelude::*;
use gtk4::{Box, Button, DrawingArea, Orientation, Overlay, Paned, ScrolledWindow, TextView, WrapMode};
use std::cell::Cell;
use std::rc::Rc;

use super::glyph::{GlyphEntry, GlyphLabel};
use bandy::{SMessage, Synapse};

// AETHERMACDEVTOOLS (SR312): the pane's rules and verbs are `widgets::devtools_rows`' (both faces read them); the
// names stay re-exported here so `devtools_view::stored_dock` and the rest keep their paths.
pub use crate::widgets::devtools_rows::{
    dock_from_pref, dock_pref, goto_target, paned_position, share_from_position, splitter_settled, store_dock, stored_dock, PANELS,
};
use crate::widgets::devtools_rows::{eval_msg, toggle_key};

/// The page and the pane, split.
pub struct DevtoolsView {
    pub root: gtk4::Widget,
    pub paned: Paned,
    pub pane: Box,
}

fn flat(label: &str, tip: &str) -> Button {
    let b = Button::new();
    b.set_child(Some(&GlyphLabel::new(label)));
    b.add_css_class("flat");
    b.set_tooltip_text(Some(tip));
    b
}

/// Builds the split around `page` (the page view with its overlays) and wires it to the bus. The dock
/// side Principia holds is sent to the shell once, here.
pub fn bootstrap_devtools(synapse: Synapse, page: &gtk4::Widget) -> DevtoolsView {
    // The pane: the panel tabs, dock and close; the text; the Console field.
    let pane = Box::new(Orientation::Vertical, 0);
    pane.add_css_class("view");
    let head = Box::new(Orientation::Horizontal, 2);
    for label in PANELS {
        let b = flat(label, label);
        let syn = synapse.clone();
        b.connect_clicked(move |_| syn.fire(SMessage::BrowserDevtoolsPanel(label.to_string())));
        head.append(&b);
    }
    let spacer = Box::new(Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    head.append(&spacer);
    let dock = flat("⇵", "Dock to the other side");
    let close = flat("×", "Close (F12)");
    head.append(&dock);
    head.append(&close);
    pane.append(&head);

    let text = TextView::new();
    text.set_editable(false);
    text.set_cursor_visible(false);
    text.set_monospace(true);
    text.set_wrap_mode(WrapMode::None);
    {
        // AETHERDEVTOOLS3 (SR128): a click on a Console frame line (`    at fn (url:line:col)`) opens the
        // Sources panel at that line — the engine's `DevTools::goto` through `BrowserDevtoolsGoto`.
        let syn = synapse.clone();
        let tv = text.clone();
        let click = gtk4::GestureClick::new();
        click.connect_released(move |_, _, x, y| {
            let (bx, by) = tv.window_to_buffer_coords(gtk4::TextWindowType::Widget, x as i32, y as i32);
            if let Some(iter) = tv.iter_at_location(bx, by) {
                let buf = tv.buffer();
                let start = buf.iter_at_line(iter.line()).unwrap_or(iter);
                let mut end = start;
                if !end.ends_line() {
                    end.forward_to_line_end();
                }
                let line = buf.text(&start, &end, false).to_string();
                if let Some((url, line)) = goto_target(&line) {
                    syn.fire(SMessage::BrowserDevtoolsGoto { url, line });
                }
            }
        });
        text.add_controller(click);
    }
    let scroll = ScrolledWindow::new();
    scroll.set_child(Some(&text));
    scroll.set_vexpand(true);
    scroll.set_hexpand(true);
    pane.append(&scroll);

    let eval = GlyphEntry::new("Evaluate in the page (Enter)");
    eval.set_margin_start(4);
    eval.set_margin_end(4);
    eval.set_margin_top(2);
    eval.set_margin_bottom(2);
    {
        let syn = synapse.clone();
        eval.connect_activate(move |e| {
            if let Some(m) = eval_msg(&e.text()) {
                syn.fire(m);
                e.set_text("");
            }
        });
    }
    pane.append(&eval);
    pane.set_visible(false);

    // The split: the page keeps whatever the pane does not take.
    let paned = Paned::new(Orientation::Horizontal);
    paned.set_start_child(Some(page));
    paned.set_end_child(Some(&pane));
    paned.set_resize_start_child(true);
    paned.set_resize_end_child(false);
    paned.set_shrink_start_child(false);
    paned.set_shrink_end_child(false);
    paned.set_wide_handle(true);
    paned.set_hexpand(true);
    paned.set_vexpand(true);

    // The sizer: the content area's size (page + pane) is what the shell splits.
    let sizer = DrawingArea::new();
    sizer.set_hexpand(true);
    sizer.set_vexpand(true);
    sizer.set_can_target(false);
    {
        let syn = synapse.clone();
        sizer.connect_resize(move |_, w, h| {
            if w > 0 && h > 0 {
                syn.fire(SMessage::BrowserResize(w as u32, h as u32, 0));
            }
        });
    }
    let overlay = Overlay::new();
    overlay.set_child(Some(&sizer));
    overlay.add_overlay(&paned);
    overlay.set_measure_overlay(&paned, true);
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);

    // Face state: the dock side shown, the layout being applied (its own position notify is not a drag),
    // the last share sent.
    let bottom = Rc::new(Cell::new(stored_dock()));
    let applying = Rc::new(Cell::new(false));
    let last_share = Rc::new(Cell::new(0u32));
    synapse.fire(SMessage::BrowserDevtoolsDock { bottom: bottom.get() });

    {
        let (syn, bottom, applying, last_share, pane) = (synapse.clone(), bottom.clone(), applying.clone(), last_share.clone(), pane.clone());
        paned.connect_position_notify(move |p| {
            if applying.get() || !pane.is_visible() {
                return;
            }
            let total = if bottom.get() { p.height() } else { p.width() };
            let share = share_from_position(p.position(), total);
            if total > 0 && share != last_share.get() {
                last_share.set(share);
                syn.fire(SMessage::BrowserDevtoolsShare(share));
            }
        });
    }
    {
        let (syn, bottom) = (synapse.clone(), bottom.clone());
        dock.connect_clicked(move |_| {
            let want = !bottom.get();
            store_dock(want);
            syn.fire(SMessage::BrowserDevtoolsDock { bottom: want });
        });
    }
    {
        let syn = synapse.clone();
        close.connect_clicked(move |_| {
            syn.fire(toggle_key());
        });
    }

    let mut rx = synapse.subscribe();
    let (paned_c, pane_c) = (paned.clone(), pane.clone());
    gtk4::glib::spawn_future_local(async move {
        loop {
            match rx.recv().await {
                Ok(SMessage::BrowserDevtoolsLayout { bottom: b, page, pane: rect }) => {
                    bottom.set(b);
                    let orientation = if b { Orientation::Vertical } else { Orientation::Horizontal };
                    applying.set(true);
                    if paned_c.orientation() != orientation {
                        paned_c.set_orientation(orientation);
                    }
                    match rect {
                        None => pane_c.set_visible(false),
                        Some(_) => {
                            pane_c.set_visible(true);
                            let total = if b { paned_c.height() } else { paned_c.width() };
                            let target = paned_position(b, page);
                            if !splitter_settled(paned_c.position(), target, total) {
                                paned_c.set_position(target);
                            }
                            last_share.set(share_from_position(target, total));
                        }
                    }
                    applying.set(false);
                }
                Ok(SMessage::BrowserDevtoolsLines(lines)) => {
                    text.buffer().set_text(&lines.join("\n"));
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });

    DevtoolsView { root: overlay.upcast(), paned, pane }
}
