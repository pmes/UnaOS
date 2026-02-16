// libs/quartzite/src/backend.rs
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow};
use async_channel::Receiver;
use std::rc::Rc;
use std::cell::RefCell;
use log::info;
use std::time::Instant;

use gneiss_pal::{AppHandler, Event, GuiUpdate}; // Import from logic kernel

pub struct Backend<A: AppHandler> {
    app_handler: Rc<RefCell<A>>,
    app_id: String,
}

impl<A: AppHandler> Backend<A> {
    pub fn new<F>(app_id: &str, app_handler: A, rx: Receiver<GuiUpdate>, bootstrap_fn: F) -> Self
    where F: Fn(&ApplicationWindow, async_channel::Sender<Event>, Receiver<GuiUpdate>) -> gtk4::Widget + 'static
    {
        // Ensure resources are registered
        crate::init();

        let app = Application::builder()
            .application_id(app_id)
            .build();

        app.connect_startup(|_| {
             if let Some(display) = gtk4::gdk::Display::default() {
                 let icon_theme = gtk4::IconTheme::for_display(&display);
                 icon_theme.add_resource_path("/org/una/vein/icons");
             }
        });

        let app_handler_rc = Rc::new(RefCell::new(app_handler));

        // BRIDGE: Event Channel -> AppHandler
        let (tx_event, rx_event) = async_channel::unbounded::<Event>();
        let handler_clone_for_bridge = app_handler_rc.clone();

        glib::MainContext::default().spawn_local(async move {
            while let Ok(event) = rx_event.recv().await {
                handler_clone_for_bridge.borrow_mut().handle_event(event);
            }
        });

        // UI BOOTSTRAP
        let bootstrap_rc = Rc::new(bootstrap_fn);
        let rx_clone = rx.clone(); // Pass RX to UI for local updates (Console, Status)

        app.connect_activate(move |app| {
            let ui_build_start_time = Instant::now();
            info!("UI_BUILD: Starting build_ui function.");

            let window = ApplicationWindow::builder()
                .application(app)
                .default_width(1200)
                .default_height(800)
                .title("Vein (Trinity)")
                .build();

            // Call the Spline Bootstrap
            let content = (bootstrap_rc)(&window, tx_event.clone(), rx_clone.clone());
            window.set_child(Some(&content));

            window.present();
            info!("UI_BUILD: Window presented. Duration: {:?}", ui_build_start_time.elapsed());
        });

        app.run();

        Self {
            app_handler: app_handler_rc,
            app_id: app_id.to_string(),
        }
    }
}
