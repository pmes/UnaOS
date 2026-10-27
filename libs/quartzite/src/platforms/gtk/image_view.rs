use gtk4::prelude::*;
use gtk4::DrawingArea;
use std::cell::RefCell;
use std::rc::Rc;
use bandy::{SMessage, Synapse};
use crate::NativeView;

pub fn bootstrap_image_surface(id: &str, synapse: Synapse) -> NativeView {
    let drawing_area = DrawingArea::builder()
        .hexpand(true)
        .vexpand(true)
        .build();
    
    let buffer = Rc::new(RefCell::new(Vec::<u8>::new()));
    let buffer_clone = buffer.clone();
    let dim = Rc::new(RefCell::new((0u32, 0u32)));
    let dim_clone = dim.clone();

    drawing_area.set_draw_func(move |_area, cr, width, height| {
        let b = buffer_clone.borrow();
        let d = dim_clone.borrow();
        if !b.is_empty() && d.0 > 0 && d.1 > 0 {
            // Aether debug: verify viewport dimensions
            if std::env::var("AETHER_DEBUG").is_ok() {
                println!("GTK draw_func: surface {}x{}, buffer len {}", d.0, d.1, b.len());
            }
            let stride = (d.0 * 4) as i32;
            if let Ok(surface) = gtk4::cairo::ImageSurface::create_for_data(
                b.to_vec(),
                gtk4::cairo::Format::ARgb32,
                d.0 as i32,
                d.1 as i32,
                stride,
            ) {
                let _ = cr.set_source_surface(&surface, 0.0, 0.0);
                let _ = cr.paint();
            }
        } else {
            cr.set_source_rgb(1.0, 1.0, 1.0);
            let _ = cr.paint();
        }
    });

    // SR67: pointer motion over the page surface goes to the engine in surface pixels (the frame
    // is painted 1:1 at the widget origin), for hover, `:hover` and the page's mouse events.
    let motion = gtk4::EventControllerMotion::new();
    let motion_syn = synapse.clone();
    motion.connect_motion(move |_, x, y| {
        motion_syn.fire(SMessage::BrowserMouseMove(x, y));
    });
    drawing_area.add_controller(motion);
    // AETHERINPUT (SR68): the surface takes the page's input. A click (fired on release) focuses the
    // surface and reaches the engine; every key goes as `BrowserKey` — its keysym name with `Ctrl+`,
    // `Alt+`, `Shift+`, `Meta+` prefixes — and an unmodified printable key also as `BrowserText`;
    // Ctrl+V reads the clipboard and sends its text as `BrowserPaste`.
    drawing_area.set_focusable(true);
    drawing_area.set_can_focus(true);
    {
        let click = gtk4::GestureClick::new();
        let syn = synapse.clone();
        let da = drawing_area.clone();
        click.connect_released(move |_, _n, x, y| {
            da.grab_focus();
            syn.fire(SMessage::BrowserClick(x, y));
        });
        drawing_area.add_controller(click); { let drag = gtk4::GestureDrag::new(); let s = synapse.clone(); drag.connect_drag_begin(move |g, x, y| s.fire(SMessage::BrowserButton { down: true, x, y, mods: button_mods(g.current_event_state()) })); let s = synapse.clone(); drag.connect_drag_end(move |g, dx, dy| { if let Some((x, y)) = g.start_point() { s.fire(SMessage::BrowserButton { down: false, x: x + dx, y: y + dy, mods: button_mods(g.current_event_state()) }); } }); drawing_area.add_controller(drag); } // NLE2B (SR208): the press and the release (BrowserButton) for every drag; GestureClick sends no release past its threshold
    }
    {
        let key = gtk4::EventControllerKey::new();
        let syn = synapse.clone();
        let da = drawing_area.clone();
        key.connect_key_pressed(move |_, keyval, _code, mods| {
            use gtk4::gdk::{Key, ModifierType};
            let ctrl = mods.contains(ModifierType::CONTROL_MASK);
            let alt = mods.contains(ModifierType::ALT_MASK);
            let shift = mods.contains(ModifierType::SHIFT_MASK);
            let meta = mods.intersects(ModifierType::META_MASK | ModifierType::SUPER_MASK);
            // AETHERSELECT (SR69): Ctrl/Cmd+C copies the page selection, Ctrl/Cmd+A selects the document.
            if (ctrl || meta) && matches!(keyval, Key::c | Key::C) {
                syn.fire(SMessage::BrowserCopy);
                return gtk4::glib::Propagation::Stop;
            }
            if (ctrl || meta) && matches!(keyval, Key::a | Key::A) {
                syn.fire(SMessage::BrowserSelectAll);
                return gtk4::glib::Propagation::Stop;
            }
            if (ctrl || meta) && matches!(keyval, Key::v | Key::V) {
                let syn = syn.clone();
                da.clipboard().read_text_async(None::<&gtk4::gio::Cancellable>, move |r| {
                    if let Ok(Some(text)) = r {
                        syn.fire(SMessage::BrowserPaste(text.to_string()));
                    }
                });
                return gtk4::glib::Propagation::Stop;
            }
            // AETHERCHROME (SR84): Esc stops the load in flight — and still reaches the page as a key.
            if keyval == Key::Escape && !ctrl && !alt && !meta {
                syn.fire(SMessage::BrowserNavStop);
            }
            let Some(name) = keyval.name() else { return gtk4::glib::Propagation::Proceed };
            let mut key = String::new();
            for (on, p) in [(ctrl, "Ctrl+"), (alt, "Alt+"), (shift, "Shift+"), (meta, "Meta+")] {
                if on {
                    key.push_str(p);
                }
            }
            key.push_str(name.as_str());
            syn.fire(SMessage::BrowserKey(key));
            if !ctrl && !alt && !meta {
                if let Some(c) = keyval.to_unicode().filter(|c| !c.is_control()) {
                    syn.fire(SMessage::BrowserText(c.to_string()));
                }
            }
            gtk4::glib::Propagation::Stop
        });
        drawing_area.add_controller(key);
    }

    let da_clone = drawing_area.clone();
    let mut rx = synapse.subscribe();
    
    let target_id = id.to_string();
    gtk4::glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            match msg {
                SMessage::SurfaceBlit { url, width, height, pixels } if url == target_id => {
                    // NLE2ORDER (SR227): the Surface takes RGBA (tetra's contract); converted ONCE here to cairo's ARgb32.
                    *buffer.borrow_mut() = crate::tetra::surface_argb32(&pixels);
                    *dim.borrow_mut() = (width, height);
                    da_clone.queue_draw();
                }
                // SR67: the hovered element's css-ui-4 cursor keyword IS the GTK cursor name
                // (GTK uses the CSS names, as `glyph.rs` does with "text"); `auto` is the arrow.
                SMessage::BrowserCursorChanged(name) => {
                    let name = if name == "auto" { "default" } else { name.as_str() };
                    da_clone.set_cursor_from_name(Some(name));
                }
                // The engine's answer to BrowserCopy: the display's clipboard.
                SMessage::BrowserClipboardSet(text) => da_clone.clipboard().set_text(&text),
                _ => {}
            }
        }
    });

    drawing_area.into()
}

/// AETHERCHROME (SR84): the page surface with the chrome that floats over it — the find bar (Ctrl+F,
/// top right), the status line (the hovered link, bottom left) and the download progress rows (bottom
/// right). Overlays only: the page keeps its full height, and the status line takes no pointer input,
/// so hovering under it still reaches the page.
pub struct PageView {
    pub root: gtk4::Widget,
    /// The page surface itself (focus returns here when the find bar closes).
    pub surface: gtk4::Widget,
    pub find_bar: gtk4::Box,
    pub find_entry: super::glyph::GlyphEntry,
}

pub fn bootstrap_page_view(id: &str, synapse: Synapse) -> PageView {
    use super::glyph::{GlyphEntry, GlyphLabel};
    // ONETEXT (SR210): the find verbs, the find count and a download row's text and bar are chrome_rows' (one copy).
    use crate::widgets::chrome_rows;
    use gtk4::{Align, Box, Button, Orientation, Overlay, ProgressBar};

    let surface = bootstrap_image_surface(id, synapse.clone());
    let overlay = Overlay::new();
    overlay.set_child(Some(&surface));
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);

    // The status line.
    let status = Box::new(Orientation::Horizontal, 0);
    status.add_css_class("osd");
    status.set_halign(Align::Start);
    status.set_valign(Align::End);
    status.set_can_target(false);
    status.set_visible(false);
    let status_label = GlyphLabel::new("");
    status_label.set_margin_start(6);
    status_label.set_margin_end(6);
    status.append(&status_label);
    overlay.add_overlay(&status);

    // The find bar.
    let find_bar = Box::new(Orientation::Horizontal, 4);
    find_bar.add_css_class("osd");
    find_bar.set_halign(Align::End);
    find_bar.set_valign(Align::Start);
    find_bar.set_margin_top(6);
    find_bar.set_margin_end(6);
    find_bar.set_visible(false);
    let find_entry = GlyphEntry::new("Find in page");
    find_entry.set_size_request(220, -1);
    let find_count = GlyphLabel::new("");
    let flat = |label: &str, tip: &str| {
        let b = Button::new();
        b.set_child(Some(&GlyphLabel::new(label)));
        b.add_css_class("flat");
        b.set_tooltip_text(Some(tip));
        b
    };
    let prev = flat("↑", "Previous match");
    let next = flat("↓", "Next match (Enter)");
    let close = flat("×", "Close (Esc)");
    find_bar.append(&find_entry);
    find_bar.append(&find_count);
    find_bar.append(&prev);
    find_bar.append(&next);
    find_bar.append(&close);
    overlay.add_overlay(&find_bar);
    {
        let syn = synapse.clone();
        find_entry.connect_activate(move |e| syn.fire(chrome_rows::find_msg(&e.text(), true)));
        let (syn, e) = (synapse.clone(), find_entry.clone());
        next.connect_clicked(move |_| syn.fire(chrome_rows::find_msg(&e.text(), true)));
        let (syn, e) = (synapse.clone(), find_entry.clone());
        prev.connect_clicked(move |_| syn.fire(chrome_rows::find_msg(&e.text(), false)));
        let hide = {
            let (syn, bar, surf, count) = (synapse.clone(), find_bar.clone(), surface.clone(), find_count.clone());
            std::rc::Rc::new(move || {
                bar.set_visible(false);
                count.set_text("");
                syn.fire(SMessage::BrowserFindClose);
                surf.grab_focus();
            })
        };
        let h = hide.clone();
        close.connect_clicked(move |_| h());
        let key = gtk4::EventControllerKey::new();
        key.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gtk4::gdk::Key::Escape {
                hide();
                return gtk4::glib::Propagation::Stop;
            }
            gtk4::glib::Propagation::Proceed
        });
        find_bar.add_controller(key);
    }

    // The downloads: one row per download, newest at the bottom.
    let downloads = Box::new(Orientation::Vertical, 4);
    downloads.set_halign(Align::End);
    downloads.set_valign(Align::End);
    downloads.set_margin_end(6);
    downloads.set_margin_bottom(6);
    overlay.add_overlay(&downloads);
    let rows: std::rc::Rc<RefCell<std::collections::HashMap<u64, (GlyphLabel, ProgressBar)>>> = Default::default();

    let mut rx = synapse.subscribe();
    let find_count_c = find_count.clone();
    gtk4::glib::spawn_future_local(async move {
        loop {
            match rx.recv().await {
                Ok(SMessage::BrowserStatus(text)) => {
                    status.set_visible(!text.is_empty());
                    status_label.set_text(&text);
                }
                Ok(SMessage::BrowserFindResult { matches, current, .. }) => {
                    find_count_c.set_text(&chrome_rows::find_count(matches, current));
                }
                Ok(SMessage::BrowserDownload { id, name, received, total, done, error }) => {
                    let mut map = rows.borrow_mut();
                    let (label, bar) = map
                        .entry(id)
                        .or_insert_with(|| {
                            let row = Box::new(Orientation::Vertical, 2);
                            row.add_css_class("osd");
                            let head = Box::new(Orientation::Horizontal, 4);
                            let label = GlyphLabel::new("");
                            label.set_hexpand(true);
                            let dismiss = Button::new();
                            dismiss.set_child(Some(&GlyphLabel::new("×")));
                            dismiss.add_css_class("flat");
                            let (list, r) = (downloads.clone(), row.clone());
                            dismiss.connect_clicked(move |_| list.remove(&r));
                            head.append(&label);
                            head.append(&dismiss);
                            let bar = ProgressBar::new();
                            row.append(&head);
                            row.append(&bar);
                            row.set_size_request(280, -1);
                            downloads.append(&row);
                            (label, bar)
                        })
                        .clone();
                    label.set_text(&chrome_rows::download_text(&name, received, total, done, error.as_deref()));
                    match chrome_rows::download_fraction(received, total, done, done && error.is_some()) {
                        Some(f) => bar.set_fraction(f),
                        None => bar.pulse(),
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });

    PageView { root: overlay.upcast(), surface, find_bar, find_entry }
}

/// NLE2-MODS (LEDGER SR306): gdk's modifier state as `BrowserButton`'s `mods` bits (bandy's `MOD_*`), read the way the
/// surface's key handler reads it (Meta or Super is the super bit).
fn button_mods(m: gtk4::gdk::ModifierType) -> u8 {
    use gtk4::gdk::ModifierType as M;
    [
        (M::SHIFT_MASK, bandy::MOD_SHIFT),
        (M::CONTROL_MASK, bandy::MOD_CONTROL),
        (M::ALT_MASK, bandy::MOD_ALT),
        (M::META_MASK | M::SUPER_MASK, bandy::MOD_SUPER),
    ]
    .into_iter()
    .filter(|(mask, _)| m.intersects(*mask))
    .fold(0, |bits, (_, bit)| bits | bit)
}
