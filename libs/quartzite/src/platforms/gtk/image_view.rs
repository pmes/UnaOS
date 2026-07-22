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
