// libs/quartzite/src/backend.rs
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow};
use async_channel::Receiver;
use std::rc::Rc;
use std::cell::RefCell;
use log::info;
use std::time::Instant;

#[cfg(feature = "gnome")]
use libadwaita::prelude::*;
#[cfg(feature = "gnome")]
use libadwaita as adw;

use gneiss_pal::{AppHandler, Event, GuiUpdate};

pub struct Backend<A: AppHandler> {
    app_handler: Rc<RefCell<A>>,
    app_id: String,
}

impl<A: AppHandler> Backend<A> {
    // We relax the window type to IsA<ApplicationWindow> to support both GTK and Adwaita
    pub fn new<F>(app_id: &str, app_handler: A, rx: Receiver<GuiUpdate>, bootstrap_fn: F) -> Self
    where F: Fn(&ApplicationWindow, async_channel::Sender<Event>, Receiver<GuiUpdate>) -> gtk4::Widget + 'static
    {
        crate::init();

        #[cfg(feature = "gnome")]
        let app = adw::Application::builder().application_id(app_id).build();
        #[cfg(not(feature = "gnome"))]
        let app = Application::builder().application_id(app_id).build();

        app.connect_startup(|_| {
             if let Some(display) = gtk4::gdk::Display::default() {
                 let icon_theme = gtk4::IconTheme::for_display(&display);
                 icon_theme.add_resource_path("/org/una/vein/icons");
             }
        });

        let app_handler_rc = Rc::new(RefCell::new(app_handler));
        let (tx_event, rx_event) = async_channel::unbounded::<Event>();
        let handler_clone_for_bridge = app_handler_rc.clone();

        glib::MainContext::default().spawn_local(async move {
            while let Ok(event) = rx_event.recv().await {
                handler_clone_for_bridge.borrow_mut().handle_event(event);
            }
        });

        let bootstrap_rc = Rc::new(bootstrap_fn);
        let rx_clone = rx.clone();

        app.connect_activate(move |app| {
            let ui_build_start_time = Instant::now();
            info!("UI_BUILD: Starting build_ui function.");

            #[cfg(feature = "gnome")]
            let window = adw::ApplicationWindow::builder()
                .application(app)
                .default_width(1200)
                .default_height(800)
                .title("Vein (Trinity)")
                .build();

            #[cfg(not(feature = "gnome"))]
            let window = ApplicationWindow::builder()
                .application(app)
                .default_width(1200)
                .default_height(800)
                .title("Vein (Trinity)")
                .build();

            // Cast to generic ApplicationWindow for the callback
            let generic_window = window.upcast_ref::<ApplicationWindow>();
            let content = (bootstrap_rc)(generic_window, tx_event.clone(), rx_clone.clone());

            #[cfg(feature = "gnome")]
            window.set_content(Some(&content));
            #[cfg(not(feature = "gnome"))]
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
