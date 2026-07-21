use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Box, Entry, DrawingArea, Orientation};
use std::cell::RefCell;
use std::rc::Rc;
use aether::AetherEngine;

pub fn run() {
    let app = Application::builder()
        .application_id("com.unaos.aether")
        .build();

    app.connect_activate(build_ui);
    app.run();
}

fn build_ui(app: &Application) {
    let window = ApplicationWindow::builder()
        .application(app)
        .title("Aether Browser")
        .default_width(800)
        .default_height(600)
        .build();

    let vbox = Box::new(Orientation::Vertical, 0);

    let url_bar = Entry::builder()
        .placeholder_text("Enter URL...")
        .build();

    let drawing_area = DrawingArea::builder()
        .hexpand(true)
        .vexpand(true)
        .build();

    vbox.append(&url_bar);
    vbox.append(&drawing_area);
    window.set_child(Some(&vbox));

    let engine = Rc::new(RefCell::new(AetherEngine::new()));
    let rt = Rc::new(tokio::runtime::Runtime::new().unwrap());
    
    let engine_clone = engine.clone();
    let da_clone = drawing_area.clone();
    let handle = rt.handle().clone();
    
    url_bar.connect_activate(move |entry| {
        let url = entry.text().to_string();
        let engine_ref = engine_clone.clone();
        let da = da_clone.clone();
        let handle_clone = handle.clone();
        
        glib::MainContext::default().spawn_local(async move {
            let _guard = handle_clone.enter();
            if let Err(e) = engine_ref.borrow_mut().load_url(&url).await {
                eprintln!("Load error: {}", e);
            }
            da.queue_draw();
        });
    });

    let engine_draw = engine.clone();
    drawing_area.set_draw_func(move |_area, cr, width, height| {
        let w = width as u32;
        let h = height as u32;
        let mut buf = vec![0; (w * h * 4) as usize];
        
        engine_draw.borrow_mut().render_frame(&mut buf, w, h);
        
        if let Ok(mut surface) = gtk4::cairo::ImageSurface::create(gtk4::cairo::Format::ARgb32, w as i32, h as i32) {
            {
                let mut data = surface.data().unwrap();
                data.copy_from_slice(&buf);
            }
            cr.set_source_surface(&surface, 0.0, 0.0).unwrap();
            cr.paint().unwrap();
        }
    });

    window.present();
    
    // Keep runtime alive
    std::mem::forget(rt);
}
