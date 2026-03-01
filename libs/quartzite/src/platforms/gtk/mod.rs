#![cfg(not(target_os = "macos"))]

use gtk4::prelude::*;
#[cfg(feature = "gnome")]
use libadwaita as adw;
#[cfg(feature = "gnome")]
use libadwaita::prelude::*;

#[allow(unused_imports)]
use gtk4::{Application, ApplicationWindow};
use log::info;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use crate::{NativeWindow, NativeView};

pub mod spline;

pub struct Backend {
    #[cfg(feature = "gnome")]
    app: adw::Application,
    #[cfg(not(feature = "gnome"))]
    app: Application,
}

impl Backend {
    // S41: Simplified Signature.
    pub fn new<F>(app_id: &str, bootstrap_fn: F) -> Self
    where
        F: FnOnce(&NativeWindow) -> NativeView + 'static,
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

        // We wrap the bootstrap_fn in an Option/RefCell to allow FnOnce execution
        let bootstrap_option = Rc::new(RefCell::new(Some(bootstrap_fn)));

        app.connect_activate(move |app| {
            // S41 Fix: Check if a window already exists.
            // If so, present it and return early. This prevents re-running
            // the bootstrap (which is FnOnce) on subsequent activation events.
            if let Some(window) = app.active_window() {
                window.present();
                return;
            }

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

            // Execute bootstrap if available
            if let Some(bootstrap) = bootstrap_option.borrow_mut().take() {
                 let content: NativeView = (bootstrap)(&window);

                #[cfg(feature = "gnome")]
                window.set_content(Some(&content));
                #[cfg(not(feature = "gnome"))]
                window.set_child(Some(&content));
            } else {
                // Should not happen if app.active_window() works correctly,
                // but if we somehow get here without a bootstrap, log error.
                info!("UI_BUILD: Bootstrap closure already consumed!");
            }

            window.present();
            info!(
                "UI_BUILD: Window presented. Duration: {:?}",
                ui_build_start_time.elapsed()
            );
        });

        Self { app }
    }

    pub fn run(&self) {
        self.app.run();
    }
}
