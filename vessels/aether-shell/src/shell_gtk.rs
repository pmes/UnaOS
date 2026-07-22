use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Box, Entry, DrawingArea, Orientation, Button, EventControllerScroll, EventControllerKey, GestureClick, EventControllerMotion};
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

    let hbox = Box::new(Orientation::Horizontal, 5);
    
    let back_btn = Button::builder().label("<").build();
    let fwd_btn = Button::builder().label(">").build();
    let url_bar = Entry::builder()
        .placeholder_text("Enter URL...")
        .hexpand(true)
        .build();

    hbox.append(&back_btn);
    hbox.append(&fwd_btn);
    hbox.append(&url_bar);

    let drawing_area = DrawingArea::builder()
        .hexpand(true)
        .vexpand(true)
        .can_focus(true)
        .build();

    vbox.append(&hbox);
    vbox.append(&drawing_area);
    window.set_child(Some(&vbox));

    let engine = Rc::new(RefCell::new(AetherEngine::new()));
    let rt = Rc::new(tokio::runtime::Runtime::new().unwrap());
    
    let engine_clone = engine.clone();
    let da_clone = drawing_area.clone();
    let handle = rt.handle().clone();
    
    let handle_url = handle.clone();
    url_bar.connect_activate(move |entry| {
        let url = entry.text().to_string();
        let engine_ref = engine_clone.clone();
        let da = da_clone.clone();
        let handle_clone = handle_url.clone();
        
        glib::MainContext::default().spawn_local(async move {
            let html = {
                let _guard = handle_clone.enter();
                match aether::net::fetch_document(&url).await {
                    Ok(content) => content,
                    Err(e) => format!(
                        "<html><head><title>Error</title></head><body style=\"background-color: #f8d7da; color: #721c24; padding: 20px; font-family: sans-serif;\"><h1>Navigation Error</h1><p>Failed to load {}: {}</p></body></html>",
                        url, e
                    ),
                }
            };
            engine_ref.borrow_mut().load_html(&url, &html, true);
            da.queue_draw();
        });
    });

    let engine_back = engine.clone();
    let da_back = drawing_area.clone();
    let handle_back = handle.clone();
    back_btn.connect_clicked(move |_| {
        let engine_ref = engine_back.clone();
        let da = da_back.clone();
        let handle_clone = handle_back.clone();
        glib::MainContext::default().spawn_local(async move {
            let url = engine_ref.borrow_mut().get_back_url();
            if let Some(u) = url {
                let html = {
                    let _guard = handle_clone.enter();
                    match aether::net::fetch_document(&u).await {
                        Ok(content) => content,
                        Err(e) => format!("<html><body><h1>Error</h1><p>{}</p></body></html>", e),
                    }
                };
                engine_ref.borrow_mut().load_html(&u, &html, false);
                da.queue_draw();
            }
        });
    });

    let engine_fwd = engine.clone();
    let da_fwd = drawing_area.clone();
    let handle_fwd = handle.clone();
    fwd_btn.connect_clicked(move |_| {
        let engine_ref = engine_fwd.clone();
        let da = da_fwd.clone();
        let handle_clone = handle_fwd.clone();
        glib::MainContext::default().spawn_local(async move {
            let url = engine_ref.borrow_mut().get_forward_url();
            if let Some(u) = url {
                let html = {
                    let _guard = handle_clone.enter();
                    match aether::net::fetch_document(&u).await {
                        Ok(content) => content,
                        Err(e) => format!("<html><body><h1>Error</h1><p>{}</p></body></html>", e),
                    }
                };
                engine_ref.borrow_mut().load_html(&u, &html, false);
                da.queue_draw();
            }
        });
    });

    // Drawing Area Events
    let scroll_ctl = EventControllerScroll::new(gtk4::EventControllerScrollFlags::VERTICAL | gtk4::EventControllerScrollFlags::HORIZONTAL);
    let engine_scroll = engine.clone();
    let da_scroll = drawing_area.clone();
    scroll_ctl.connect_scroll(move |_ctl, dx, dy| {
        engine_scroll.borrow_mut().handle_event(aether::api::events::Event::Scroll(dx * 40.0, dy * 40.0));
        da_scroll.queue_draw();
        glib::Propagation::Stop
    });
    drawing_area.add_controller(scroll_ctl);
    
    let engine_resize = engine.clone();
    drawing_area.connect_resize(move |da, width, height| {
        engine_resize.borrow_mut().handle_event(aether::api::events::Event::Resize(width as u32, height as u32));
        da.queue_draw();
    });

    let key_ctl = EventControllerKey::new();
    let engine_key = engine.clone();
    let da_key = drawing_area.clone();
    
    let im_context = gtk4::IMMulticontext::new();
    let engine_im = engine.clone();
    let da_im = drawing_area.clone();
    im_context.connect_commit(move |_im, text| {
        engine_im.borrow_mut().handle_event(aether::api::events::Event::Text(text.to_string()));
        da_im.queue_draw();
    });
    
    let im_context_clone = im_context.clone();
    key_ctl.connect_key_pressed(move |ctl, keyval, _keycode, _state| {
        let ev = ctl.current_event().unwrap();
        if im_context_clone.filter_keypress(&ev) {
            return glib::Propagation::Stop;
        }
        if let Some(name) = keyval.name() {
            engine_key.borrow_mut().handle_event(aether::api::events::Event::KeyDown(name.to_string()));
        }
        da_key.queue_draw();
        glib::Propagation::Proceed
    });
    drawing_area.add_controller(key_ctl);

    let engine_draw = engine.clone();
    drawing_area.set_draw_func(move |_area, cr, width, height| {
        let w = width as u32;
        let h = height as u32;
        
        let mut eng = engine_draw.borrow_mut();
        eng.render_frame();
        
        
        if let Ok(mut surface) = gtk4::cairo::ImageSurface::create(gtk4::cairo::Format::ARgb32, w as i32, h as i32) {
            {
                let mut data = surface.data().unwrap();
                let src = eng.surface();
                let len = src.len().min(data.len());
                data[..len].copy_from_slice(&src[..len]);
            }
            cr.set_source_surface(&surface, 0.0, 0.0).unwrap();
            cr.paint().unwrap();
        }
    });

    let engine_tick = engine.clone();
    let da_tick = drawing_area.clone();
    let handle_tick = handle.clone();
    let window_tick = window.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
        let _guard = handle_tick.enter();
        if engine_tick.borrow_mut().tick() {
            da_tick.queue_draw();
            
            let title = engine_tick.borrow().title.clone();
            if title != window_tick.title().unwrap_or_default().as_str() {
                window_tick.set_title(Some(&title));
            }
        }
        glib::ControlFlow::Continue
    });

    window.present();
    std::mem::forget(rt);
}
