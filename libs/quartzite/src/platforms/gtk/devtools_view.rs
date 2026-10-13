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
use bandy::{PrefValue, SMessage, Synapse};

/// The pane's panels, as the engine labels them (`aether::devtools::Panel::label`); AETHERDEVTOOLS3
/// (SR128) adds Sources, in the engine's tab order.
pub const PANELS: [&str; 7] = ["Elements", "Console", "Sources", "Network", "Storage", "Performance", "Accessibility"];

/// AETHERDEVTOOLS3 (SR128): the (url, line) a pane text line names when it is a V8-shaped frame
/// (`    at fn (url:line:col)`, `    at url:line:col`) — the face's half of a frame click; the engine's own
/// reading is `aether::devtools::sources::frame_target`, the same grammar. The click itself waits on a bus
/// verb carrying it to `DevTools::goto` (bandy has none yet: named in the SR128 row).
pub fn goto_target(line: &str) -> Option<(String, u32)> {
    let t = line.trim();
    let rest = t.strip_prefix("at ")?.trim_end();
    let loc = match (rest.strip_suffix(')'), rest.rfind(" (")) {
        (Some(r), Some(k)) => &r[k + 2..],
        _ => rest,
    };
    let mut it = loc.rsplitn(3, ':');
    let _col: u32 = it.next()?.parse().ok()?;
    let n: u32 = it.next()?.parse().ok()?;
    let file = it.next().filter(|f| !f.is_empty() && !f.contains(char::is_whitespace))?;
    Some((file.to_string(), n))
}

/// Principia's spelling of a dock side (`aether.devtools.dock`): true = under the page.
pub fn dock_from_pref(v: &str) -> bool {
    v.eq_ignore_ascii_case("bottom")
}

pub fn dock_pref(bottom: bool) -> &'static str {
    if bottom { "bottom" } else { "right" }
}

/// The pane's share (percent of the dock axis) for a splitter at `pos` of `total` px, rounded.
pub fn share_from_position(pos: i32, total: i32) -> u32 {
    if total <= 0 {
        return 0;
    }
    let pane = (total - pos.clamp(0, total)) as i64;
    ((pane * 100 + total as i64 / 2) / total as i64) as u32
}

/// The `Paned` position for a layout: the page rect's extent along the dock axis.
pub fn paned_position(bottom: bool, page: (u32, u32, u32, u32)) -> i32 {
    (if bottom { page.3 } else { page.2 }) as i32
}

/// Whether the splitter at `current` is already where a layout puts it (within 1 % of `total`): a
/// drag's own echo is not snapped back.
pub fn splitter_settled(current: i32, target: i32, total: i32) -> bool {
    (current - target).abs() <= (total / 100).max(1)
}

/// The dock side the person chose last (Principia `aether.devtools.dock`, else its schema default).
pub fn stored_dock() -> bool {
    let Ok(store) = principia::prefs::PrefStore::load(principia::default_prefs_path()) else { return false };
    if let Some(PrefValue::Str(s)) = store.get("aether", "devtools.dock") {
        return dock_from_pref(&s);
    }
    match store.effective("aether", "devtools.dock", |k| std::env::var(k).ok()) {
        Some((PrefValue::Str(s), _)) => dock_from_pref(&s),
        _ => false,
    }
}

/// Store the dock side in Principia (only into a store that loaded: a failed load never writes over the
/// person's other `aether` choices).
pub fn store_dock(bottom: bool) {
    match principia::prefs::PrefStore::load(principia::default_prefs_path()) {
        Ok(mut store) => {
            if let Err(e) = store.set("aether", "devtools.dock", PrefValue::Str(dock_pref(bottom).to_string())) {
                log::warn!("[aether] devtools dock not stored: {e}");
            }
        }
        Err(e) => log::warn!("[aether] devtools dock not stored (prefs did not load): {e}"),
    }
}

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
            let line = e.text();
            if !line.trim().is_empty() {
                syn.fire(SMessage::BrowserDevtoolsEval(line));
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
                syn.fire(SMessage::BrowserResize(w as u32, h as u32));
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
            syn.fire(SMessage::BrowserDevtoolsKey { key: "F12".into(), ctrl: false, shift: false, alt: false });
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The face's half of the split: a layout's page rect places the splitter, a drag at that place reads
    /// back the share the model split by, a drag's echo within 1 % is not snapped, and the dock side
    /// round-trips through Principia's spelling.
    #[test]
    fn splitter_position_and_share_round_trip() {
        // Right dock, 40 % of 1000 x 800: the page is 600 wide, the pane 400.
        assert_eq!(paned_position(false, (0, 0, 600, 800)), 600);
        assert_eq!(share_from_position(600, 1000), 40);
        // Bottom dock, 15 % of 800 high: the page is 680 high.
        assert_eq!(paned_position(true, (0, 0, 1000, 680)), 680);
        assert_eq!(share_from_position(680, 800), 15);
        assert_eq!(share_from_position(0, 0), 0);
        assert_eq!(share_from_position(-5, 100), 100);
        assert!(splitter_settled(605, 600, 1000));
        assert!(!splitter_settled(620, 600, 1000));
        assert!(dock_from_pref(dock_pref(true)) && !dock_from_pref(dock_pref(false)));
        assert!(!dock_from_pref("left"));
    }

    /// AETHERDEVTOOLS3 (SR128): the Sources tab is the third, and a frame line reads back its url and line.
    #[test]
    fn sources_tab_and_frame_targets() {
        assert_eq!(PANELS.len(), 7);
        assert_eq!(PANELS[2], "Sources");
        assert_eq!(goto_target("    at boom (https://a.test/index.html:3:9)"), Some(("https://a.test/index.html".to_string(), 3)));
        assert_eq!(goto_target("    at https://a.test/app.js:12:1"), Some(("https://a.test/app.js".to_string(), 12)));
        assert_eq!(goto_target("    at new C (https://a.test/c.js:4:20)"), Some(("https://a.test/c.js".to_string(), 4)));
        assert_eq!(goto_target("    at inner (t.js)"), None);
        assert_eq!(goto_target("x Uncaught Error: boom"), None);
    }
}
