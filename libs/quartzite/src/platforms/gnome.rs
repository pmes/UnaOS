#![allow(deprecated)]

use gtk4::prelude::*;
use libadwaita::prelude::*;
use libadwaita as adw;
use gtk4::{
    Application, Box, Orientation, Label, Button, Stack, ScrolledWindow,
    PolicyType, Align, ListBox, StackTransitionType, TextView, EventControllerKey,
    TextBuffer, Adjustment, FileChooserNative, ResponseType, FileChooserAction,
    StackSwitcher, ToggleButton, CssProvider, StyleContext, Image, MenuButton, Popover,
    Paned, Spinner, ApplicationWindow
};
use sourceview5::prelude::*;
use sourceview5::View as SourceView;
use sourceview5::{Buffer, StyleSchemeManager};
use gtk4::gdk::{Key, ModifierType};
use std::rc::Rc;
use std::cell::RefCell;
use std::time::Duration;
use log::info;
use std::time::Instant;
use std::io::Write;
use std::path::PathBuf;
use async_channel::Receiver;

use crate::types::*;
use crate::shard::{Shard, ShardRole, ShardStatus};

pub struct Backend<A: AppHandler> {
    app_handler: Rc<RefCell<A>>,
    app_id: String,
}

impl<A: AppHandler> Backend<A> {
    // S40: Updated signature to accept bootstrap_fn
    pub fn new<F>(app_id: &str, app_handler: A, rx: Receiver<GuiUpdate>, bootstrap_fn: F) -> Self
    where F: Fn(&ApplicationWindow, async_channel::Sender<Event>, Receiver<GuiUpdate>) -> gtk4::Widget + 'static
    {
        // Ensure resources are registered
        crate::register_resources();

        let app = Application::builder()
            .application_id(app_id)
            .build();

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
        let rx_clone = rx.clone(); // Clone channel receiver (async-channel is multi-consumer)

        app.connect_activate(move |app| {
            build_ui(app, rx_clone.clone(), bootstrap_rc.clone(), tx_event.clone());
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
    rx: Receiver<GuiUpdate>,
    bootstrap: Rc<F>,
    tx_event: async_channel::Sender<Event>
)
where F: Fn(&ApplicationWindow, async_channel::Sender<Event>, Receiver<GuiUpdate>) -> gtk4::Widget + 'static
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

    let content = bootstrap(gtk_window, tx_event, rx);

    // AdwApplicationWindow content
    window.set_content(Some(&content));

    window.present();
    info!("UI_BUILD: Window presented. Total build_ui duration: {:?}", ui_build_start_time.elapsed());
}
