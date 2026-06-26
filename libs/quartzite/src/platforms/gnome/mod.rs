#![allow(deprecated)]
// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use async_channel::Receiver;
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow};
use libadwaita as adw;
use libadwaita::prelude::*;
use log::info;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use gneiss_pal::{AppHandler, Event};
use bandy::SMessage;
use bandy::state::AppState;
use std::sync::{Arc, RwLock};

pub struct Backend<A: AppHandler> {
    #[allow(dead_code)]
    app_handler: Rc<RefCell<A>>,
    #[allow(dead_code)]
    app_id: String,
}

impl<A: AppHandler> Backend<A> {
    // S40: Updated signature to accept bootstrap_fn
    pub fn new<F>(app_id: &str, app_handler: A, app_state: Arc<RwLock<AppState>>, rx_synapse: Receiver<SMessage>, bootstrap_fn: F) -> Self
    where
        F: Fn(
                &ApplicationWindow,
                async_channel::Sender<Event>,
                Arc<RwLock<AppState>>,
                Receiver<SMessage>,
            ) -> gtk4::Widget
            + 'static,
    {
        let app = Application::builder().application_id(app_id).build();

        // Initialize Libadwaita
        app.connect_startup(|_| {
            adw::init().unwrap();

            // S40: Register Icon Theme Protocol
            if let Some(display) = gtk4::gdk::Display::default() {
                let icon_theme = gtk4::IconTheme::for_display(&display);
                icon_theme.add_resource_path("/org/una/vein/icons");
            }
        });

        let app_handler_rc = Rc::new(RefCell::new(app_handler));

        let (tx_event, rx_event) = async_channel::unbounded::<Event>();

        // Bridge Loop
        let handler_clone_for_bridge = app_handler_rc.clone();
        glib::MainContext::default().spawn_local(async move {
            while let Ok(event) = rx_event.recv().await {
                handler_clone_for_bridge.borrow_mut().handle_event(event);
            }
        });

        let bootstrap_rc = Rc::new(bootstrap_fn);


        app.connect_activate(move |app| {
            let app_state_clone = app_state.clone();
            let rx_synapse_clone = rx_synapse.clone();
            build_ui(
                app,
                app_state_clone,
                rx_synapse_clone,
                bootstrap_rc.clone(),
                tx_event.clone(),
            );
        });
        app.run();

        Self {
            app_handler: app_handler_rc,
            app_id: app_id.to_string(),
        }
    }
}

fn build_ui<F>(
    app: &Application,
    app_state: Arc<RwLock<AppState>>,
    rx_synapse: Receiver<SMessage>,
    bootstrap: Rc<F>,
    tx_event: async_channel::Sender<Event>,
) where
    F: Fn(&ApplicationWindow, async_channel::Sender<Event>, Arc<RwLock<AppState>>, Receiver<SMessage>) -> gtk4::Widget
        + 'static,
{
    let ui_build_start_time = Instant::now();
    info!("UI_BUILD: Starting build_ui function (Adwaita Spline).");

    // --- MAIN WINDOW (Adwaita) ---
    // AdwApplicationWindow
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .default_width(1200)
        .default_height(800)
        .title("Elessar (UnaOS)")
        .build();

    // Bootstrap returns a Widget.
    // However, AdwApplicationWindow expects content.
    // We need to cast AdwApplicationWindow to gtk::ApplicationWindow or pass it as is?
    // The signature says `&ApplicationWindow` (which is gtk::ApplicationWindow).
    // `adw::ApplicationWindow` is a subclass of `gtk::ApplicationWindow`.
    // So we can upcast.

    let gtk_window = window.upcast_ref::<gtk4::ApplicationWindow>();

    let content = bootstrap(gtk_window, tx_event, app_state, rx_synapse);

    // AdwApplicationWindow content
    window.set_content(Some(&content));

    window.present();
    info!(
        "UI_BUILD: Window presented. Total build_ui duration: {:?}",
        ui_build_start_time.elapsed()
    );
}

pub mod mega_bar;
