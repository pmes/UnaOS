#![cfg(not(target_os = "macos"))]
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

use crate::{NativeView, NativeWindow};

// pub mod spline;

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
                .title("")
                .build();

            #[cfg(not(feature = "gnome"))]
            let window = ApplicationWindow::builder()
                .application(app)
                .default_width(1200)
                .default_height(800)
                .title("")
                .build();

            // Execute bootstrap if available
            if let Some(bootstrap) = bootstrap_option.borrow_mut().take() {
                #[cfg(feature = "gnome")]
                let content: NativeView =
                    (bootstrap)(window.upcast_ref::<gtk4::ApplicationWindow>());
                #[cfg(not(feature = "gnome"))]
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

    pub fn new_vessel<F>(
        app_id: &str,
        title: &str,
        content_size: (f64, f64),
        build_view: F,
    ) -> Self
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

        let bootstrap_option = Rc::new(RefCell::new(Some(build_view)));
        let title = title.to_string();

        app.connect_activate(move |app| {
            if let Some(window) = app.active_window() {
                window.present();
                return;
            }

            #[cfg(feature = "gnome")]
            let window = adw::ApplicationWindow::builder()
                .application(app)
                .default_width(content_size.0 as i32)
                .default_height(content_size.1 as i32)
                .title(&title)
                .build();

            #[cfg(not(feature = "gnome"))]
            let window = ApplicationWindow::builder()
                .application(app)
                .default_width(content_size.0 as i32)
                .default_height(content_size.1 as i32)
                .title(&title)
                .build();

            if let Some(bootstrap) = bootstrap_option.borrow_mut().take() {
                #[cfg(feature = "gnome")]
                let content: NativeView =
                    (bootstrap)(window.upcast_ref::<gtk4::ApplicationWindow>());
                #[cfg(not(feature = "gnome"))]
                let content: NativeView = (bootstrap)(&window);

                #[cfg(feature = "gnome")]
                window.set_content(Some(&content));
                #[cfg(not(feature = "gnome"))]
                window.set_child(Some(&content));
            }

            window.present();
        });

        Self { app }
    }

    pub fn new_tetra_vessel(
        app_id: &str,
        title: &str,
        content_size: (f64, f64),
        tetra_node: crate::tetra::TetraNode,
        synapse: bandy::Synapse,
    ) -> Self {
        Self::new_vessel(app_id, title, content_size, move |_window| {
            crate::tetra::SurfaceMap::of(&tetra_node).scope(|| crate::platforms::gtk::tetra_eval::eval_tetra(tetra_node, synapse.clone())) // SURFACEID (SR332): each Surface leaf reads its number
        })
    }

    pub fn run(&self) {
        // AETHERMACCHROME (SR205): argv is the vessel's (aether-shell's `start_url`), not GApplication's: with no
        // HANDLES_OPEN a URL argument made GApplication refuse to start.
        let _ = self.app.run_with_args::<&str>(&[]);
    }
}

pub mod tetra_eval;
pub mod button;
pub mod text_field;
pub mod glyph;
pub mod image_view;
pub mod console_view;
// AETHERCHROME (SR84): the browser chrome's GTK face.
pub mod tab_strip;
pub mod url_bar;
// REALSENSE (SR92, R112): Sonde's GTK face — the probe pane (depth ramp beside colour) as surface + input + resize.
pub mod probe_view;
// AETHERDEVTOOLS2 (SR112): the developer-tools pane's face — the split beside the page, the splitter, the dock pref.
pub mod devtools_view;
// A11YFACE (SR124): Principia's `system.accessibility.*` rows, read at start and on change, sent to the shell.
pub mod a11y_prefs;

/// AETHERCHROME (SR84): the Aether browser window — the tab strip, the address row with the bookmark
/// bar, and the page with its find bar / status line / downloads overlays — and the browser's keyboard
/// shortcuts, installed on the window in the CAPTURE phase so they work whether the page, the address
/// field or nothing has focus (the page still receives Esc, Ctrl+C/A/V and every unclaimed key).
pub fn bootstrap_browser_chrome(window: &gtk4::ApplicationWindow, synapse: bandy::Synapse) -> NativeView {
    use bandy::SMessage;
    use gtk4::{CallbackAction, Orientation, PropagationPhase, Shortcut, ShortcutController, ShortcutTrigger};

    let root = gtk4::Box::new(Orientation::Vertical, 0);
    let strip = tab_strip::bootstrap_tab_strip(synapse.clone());
    let bar = url_bar::bootstrap_url_bar(synapse.clone());
    let page = image_view::bootstrap_page_view("viewport", synapse.clone());
    // AETHERDEVTOOLS2 (SR112): the page sits in the developer-tools split (the pane hidden until F12).
    let devtools = devtools_view::bootstrap_devtools(synapse.clone(), &page.root);
    root.append(&strip);
    root.append(&bar.root);
    root.append(&tab_strip::bootstrap_side_tabs(synapse.clone(), &devtools.root)); // AETHERSIDETABS (SR140): the side column beside the page (shown only in the `side` layout)

    let shortcuts = ShortcutController::new();
    shortcuts.set_propagation_phase(PropagationPhase::Capture);
    let add = |trigger: &str, action: CallbackAction| {
        if let Some(t) = ShortcutTrigger::parse_string(trigger) {
            shortcuts.add_shortcut(Shortcut::new(Some(t), Some(action)));
        }
    };
    let send = |msg: SMessage| {
        let syn = synapse.clone();
        CallbackAction::new(move |_, _| {
            syn.fire(msg.clone());
            gtk4::glib::Propagation::Stop
        })
    };
    for (trigger, msg) in [
        ("<Control>t", SMessage::BrowserTabNew),
        ("<Control>w", SMessage::BrowserTabClose(None)),
        ("<Control>Tab", SMessage::BrowserTabStep(1)),
        ("<Control>Page_Down", SMessage::BrowserTabStep(1)),
        ("<Control><Shift>Tab", SMessage::BrowserTabStep(-1)),
        ("<Control><Shift>ISO_Left_Tab", SMessage::BrowserTabStep(-1)),
        ("<Control>Page_Up", SMessage::BrowserTabStep(-1)),
        ("<Control>r", SMessage::BrowserNavReload),
        ("F5", SMessage::BrowserNavReload),
        ("<Alt>Left", SMessage::BrowserNavBack),
        ("<Alt>Right", SMessage::BrowserNavForward),
        ("<Control>d", SMessage::BrowserBookmarkToggle),
        ("<Control>plus", SMessage::BrowserZoom(1)),
        ("<Control>equal", SMessage::BrowserZoom(1)),
        ("<Control>KP_Add", SMessage::BrowserZoom(1)),
        ("<Control>minus", SMessage::BrowserZoom(-1)),
        ("<Control>KP_Subtract", SMessage::BrowserZoom(-1)),
        ("<Control>0", SMessage::BrowserZoom(0)),
        ("<Control>KP_0", SMessage::BrowserZoom(0)),
        // AETHERDEVTOOLS2 (SR112): the developer-tools toggle, taken here in the capture phase so the page
        // never sees it; the shell's `DevtoolsChrome::on_key` decides what it does.
        ("F12", SMessage::BrowserDevtoolsKey { key: "F12".into(), ctrl: false, shift: false, alt: false }),
        ("<Control><Shift>i", SMessage::BrowserDevtoolsKey { key: "I".into(), ctrl: true, shift: true, alt: false }),
        ("<Control><Shift>I", SMessage::BrowserDevtoolsKey { key: "I".into(), ctrl: true, shift: true, alt: false }),
    ] {
        add(trigger, send(msg));
    }
    // Ctrl+L: the address field.
    {
        let entry = bar.entry.clone();
        add("<Control>l", CallbackAction::new(move |_, _| {
            entry.grab_focus();
            gtk4::glib::Propagation::Stop
        }));
    }
    // Ctrl+F: the find bar.
    {
        let (find_bar, find_entry) = (page.find_bar.clone(), page.find_entry.clone());
        add("<Control>f", CallbackAction::new(move |_, _| {
            find_bar.set_visible(true);
            find_entry.grab_focus();
            gtk4::glib::Propagation::Stop
        }));
    }
    // Ctrl+Shift+B: the bookmark bar.
    {
        let bookmark_bar = bar.bookmark_bar.clone();
        add("<Control><Shift>b", CallbackAction::new(move |_, _| {
            bookmark_bar.set_visible(!bookmark_bar.is_visible());
            gtk4::glib::Propagation::Stop
        }));
    }
    // A11YFACE (SR124): F6 / Shift+F6 move focus between the chrome (the address field) and the page, as browsers
    // do; on the page surface the page's own keys work (Tab, Alt+1, H, D — `aether::dom::a11y::keyboard`).
    for trigger in ["F6", "<Shift>F6"] {
        let (entry, surface) = (bar.entry.clone(), page.surface.clone());
        add(trigger, CallbackAction::new(move |_, _| {
            if surface.has_focus() {
                entry.grab_focus();
            } else {
                surface.grab_focus();
            }
            gtk4::glib::Propagation::Stop
        }));
    }
    window.add_controller(shortcuts);
    // A11YFACE (SR124): the accessibility rows, now and on every change.
    a11y_prefs::bootstrap_a11y_prefs(synapse.clone());

    // The window title mirrors the active tab's document title.
    {
        let mut rx = synapse.subscribe();
        let window = window.clone();
        gtk4::glib::spawn_future_local(async move {
            loop {
                match rx.recv().await {
                    Ok(SMessage::BrowserTitleChanged(title)) => {
                        let t = if title.trim().is_empty() { "Aether Browser".to_string() } else { format!("{title} — Aether") };
                        window.set_title(Some(&t));
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        });
    }
    page.surface.grab_focus();
    root.upcast()
}
// pub mod spline;
// pub mod mega_bar;
// pub mod types;
// pub mod workspace;
