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
        drawing_area.add_controller(click);
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
            if (ctrl || meta) && matches!(keyval, Key::v | Key::V) {
                let syn = syn.clone();
                da.clipboard().read_text_async(None::<&gtk4::gio::Cancellable>, move |r| {
                    if let Ok(Some(text)) = r {
                        syn.fire(SMessage::BrowserPaste(text.to_string()));
                    }
                });
                return gtk4::glib::Propagation::Stop;
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
            if let SMessage::SurfaceBlit { url, width, height, pixels } = msg {
                if url == target_id {
                    *buffer.borrow_mut() = pixels;
                    *dim.borrow_mut() = (width, height);
                    da_clone.queue_draw();
                }
            }
        }
    });

    drawing_area.into()
}
