use gtk4::prelude::*;
use gtk4::{Box, Button, DrawingArea, Entry, Orientation};
use std::cell::RefCell;
use std::rc::Rc;
use bandy::{SMessage, Synapse};
use tokio::sync::broadcast::Receiver;

use crate::NativeView;

pub fn bootstrap_browser(
    _window: &crate::NativeWindow,
    tx: Synapse,
    mut rx: Receiver<SMessage>,
) -> NativeView {
    let vbox = Box::new(Orientation::Vertical, 0);

    let hbox = Box::new(Orientation::Horizontal, 5);
    
    let back_btn = Button::builder().label("<").build();
    let fwd_btn = Button::builder().label(">").build();
    let reload_btn = Button::builder().label("C").build();
    let url_bar = Entry::builder()
        .placeholder_text("Enter URL...")
        .hexpand(true)
        .build();

    hbox.append(&back_btn);
    hbox.append(&fwd_btn);
    hbox.append(&reload_btn);
    hbox.append(&url_bar);

    let drawing_area = DrawingArea::builder()
        .hexpand(true)
        .vexpand(true)
        .can_focus(true)
        .build();

    vbox.append(&hbox);
    vbox.append(&drawing_area);

    // Wire signals to tx
    let tx_url = tx.clone();
    url_bar.connect_activate(move |entry| {
        let url = entry.text().to_string();
        tx_url.fire(SMessage::OpenDocument { url });
    });

    let tx_back = tx.clone();
    back_btn.connect_clicked(move |_| {
        tx_back.fire(SMessage::BrowserNavBack);
    });

    let tx_fwd = tx.clone();
    fwd_btn.connect_clicked(move |_| {
        tx_fwd.fire(SMessage::BrowserNavForward);
    });

    let tx_reload = tx.clone();
    reload_btn.connect_clicked(move |_| {
        tx_reload.fire(SMessage::BrowserNavReload);
    });

    let scroll_ctl = gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::VERTICAL | gtk4::EventControllerScrollFlags::HORIZONTAL);
    let tx_scroll = tx.clone();
    scroll_ctl.connect_scroll(move |_ctl, dx, dy| {
        tx_scroll.fire(SMessage::BrowserScroll(dx * 40.0, dy * 40.0));
        glib::Propagation::Stop
    });
    drawing_area.add_controller(scroll_ctl);

    let tx_resize = tx.clone();
    drawing_area.connect_resize(move |_da, width, height| {
        tx_resize.fire(SMessage::BrowserResize(width as u32, height as u32));
    });

    let key_ctl = gtk4::EventControllerKey::new();
    let im_context = gtk4::IMMulticontext::new();
    let tx_text = tx.clone();
    im_context.connect_commit(move |_im, text| {
        tx_text.fire(SMessage::BrowserText(text.to_string()));
    });

    let im_context_clone = im_context.clone();
    let tx_key = tx.clone();
    key_ctl.connect_key_pressed(move |ctl, keyval, _keycode, _state| {
        let ev = ctl.current_event().unwrap();
        if im_context_clone.filter_keypress(&ev) {
            return glib::Propagation::Stop;
        }
        if let Some(name) = keyval.name() {
            tx_key.fire(SMessage::BrowserKey(name.to_string()));
        }
        glib::Propagation::Proceed
    });
    drawing_area.add_controller(key_ctl);

    let surface_state = Rc::new(RefCell::new(Vec::<u8>::new()));
    let w_state = Rc::new(RefCell::new(0u32));
    let h_state = Rc::new(RefCell::new(0u32));

    let da_clone = drawing_area.clone();
    let surf_clone = surface_state.clone();
    let w_clone = w_state.clone();
    let h_clone = h_state.clone();

    // Listen for SurfaceBlit messages
    glib::MainContext::default().spawn_local(async move {
        while let Ok(msg) = rx.recv().await {
            match msg {
                SMessage::SurfaceBlit { url: _, width, height, pixels } => {
                    *w_clone.borrow_mut() = width;
                    *h_clone.borrow_mut() = height;
                    *surf_clone.borrow_mut() = pixels;
                    da_clone.queue_draw();
                }
                _ => {}
            }
        }
    });

    // Draw func
    let surf_draw = surface_state.clone();
    let w_draw = w_state.clone();
    let h_draw = h_state.clone();
    drawing_area.set_draw_func(move |_area, cr, _width, _height| {
        let w = *w_draw.borrow();
        let h = *h_draw.borrow();
        if w == 0 || h == 0 { return; }

        let buf = surf_draw.borrow();
        
        if std::env::var("AETHER_DEBUG").is_ok() {
            println!(":: GTK DIAGNOSTIC :: draw_func w={} h={} buf.len()={}", w, h, buf.len());
        }

        if buf.is_empty() { return; }

        if let Ok(mut surface) = gtk4::cairo::ImageSurface::create(gtk4::cairo::Format::ARgb32, w as i32, h as i32) {
            {
                let mut data = surface.data().unwrap();
                let len = buf.len().min(data.len());
                data[..len].copy_from_slice(&buf[..len]);
            }
            cr.set_source_surface(&surface, 0.0, 0.0).unwrap();
            cr.paint().unwrap();
        }
    });

    vbox.upcast()
}
