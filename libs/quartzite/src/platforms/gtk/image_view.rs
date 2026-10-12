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
    // AETHERSELECT (SR69): the page's copy and select-all. The viewport takes keyboard focus when
    // pressed; Ctrl+C asks the engine for its selection (`BrowserCopy`, answered below with
    // `BrowserClipboardSet`) and Ctrl+A selects the document (`BrowserSelectAll`).
    drawing_area.set_focusable(true);
    let press = gtk4::GestureClick::new();
    let da_focus = drawing_area.clone();
    press.connect_pressed(move |_, _, _, _| {
        da_focus.grab_focus();
    });
    drawing_area.add_controller(press);
    let keys = gtk4::EventControllerKey::new();
    let key_syn = synapse.clone();
    keys.connect_key_pressed(move |_, keyval, _code, mods| {
        if !mods.contains(gtk4::gdk::ModifierType::CONTROL_MASK) {
            return gtk4::glib::Propagation::Proceed;
        }
        match keyval {
            gtk4::gdk::Key::c | gtk4::gdk::Key::C => {
                key_syn.fire(SMessage::BrowserCopy);
                gtk4::glib::Propagation::Stop
            }
            gtk4::gdk::Key::a | gtk4::gdk::Key::A => {
                key_syn.fire(SMessage::BrowserSelectAll);
                gtk4::glib::Propagation::Stop
            }
            _ => gtk4::glib::Propagation::Proceed,
        }
    });
    drawing_area.add_controller(keys);

    let da_clone = drawing_area.clone();
    let mut rx = synapse.subscribe();
    
    let target_id = id.to_string();
    gtk4::glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            match msg {
                SMessage::SurfaceBlit { url, width, height, pixels } if url == target_id => {
                    *buffer.borrow_mut() = pixels;
                    *dim.borrow_mut() = (width, height);
                    da_clone.queue_draw();
                }
                // SR67: the hovered element's css-ui-4 cursor keyword IS the GTK cursor name
                // (GTK uses the CSS names, as `glyph.rs` does with "text"); `auto` is the arrow.
                SMessage::BrowserCursorChanged(name) => {
                    let name = if name == "auto" { "default" } else { name.as_str() };
                    da_clone.set_cursor_from_name(Some(name));
                }
                SMessage::SurfaceBlit { url, width, height, pixels } => {
                    if url == target_id {
                        *buffer.borrow_mut() = pixels;
                        *dim.borrow_mut() = (width, height);
                        da_clone.queue_draw();
                    }
                }
                // The engine's answer to BrowserCopy: the display's clipboard.
                SMessage::BrowserClipboardSet(text) => da_clone.clipboard().set_text(&text),
                _ => {}
            }
        }
    });

    drawing_area.into()
}
