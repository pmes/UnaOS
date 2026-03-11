// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use anyhow::Result;
#[cfg(target_os = "linux")]
use gtk4::prelude::*;
#[cfg(target_os = "linux")]
use gtk4::{
    Box, HeaderBar, Orientation, Paned, Separator, Stack, StackSwitcher, StackTransitionType,
};
#[cfg(all(target_os = "linux", feature = "gnome"))]
use libadwaita as adw;
use std::cell::RefCell;
use std::env;
use std::path::PathBuf;
use std::rc::Rc;

use gneiss_pal::{Event, GuiUpdate};
use quartzite::{Backend, NativeView, NativeWindow};

#[cfg(target_os = "linux")]
use matrix::create_view as create_matrix_view;
#[cfg(target_os = "linux")]
use tabula::{EditorMode, TabulaView};

const APP_ID: &str = "org.unaos.UnaIDE";

fn main() -> Result<()> {
    println!(":: UNA :: WAKING UP THE FORGE...");

    // 0. Ignite the Substrate Reactor (Tokio)
    let rt = tokio::runtime::Runtime::new().expect("CRITICAL: Failed to ignite Tokio reactor");
    let _guard = rt.enter();

    // 1. Establish async_channel pairs
    let (tx_brain, rx_brain) = async_channel::unbounded::<Event>();
    let (tx_gui, rx_gui) = async_channel::unbounded::<GuiUpdate>();
    let (_tx_telemetry, rx_telemetry) = async_channel::unbounded::<bandy::SMessage>();

    // 2. Spawn central background task (Tokio)
    rt.spawn(async move {
        while let Ok(event) = rx_brain.recv().await {
            match event {
                Event::FileSelected(path) => {
                    println!("[UNA CORE] 🧠 Routing Impulse: {:?}", path);
                    // Bouncing it as EditorLoad to trigger tabula
                    let _ = tx_gui
                        .send(GuiUpdate::EditorLoad(path.to_string_lossy().to_string()))
                        .await;
                }
                _ => {}
            }
        }
    });

    let cwd = env::current_dir().unwrap_or_default();

    // 7. View & Engine Ignition
    let spline = Rc::new(quartzite::Spline::new());

    // THE FUSION
    let bootstrap = move |window: &NativeWindow| -> NativeView {
        #[cfg(target_os = "macos")]
        let view = {
            spline.bootstrap(
                window,
                tx_brain.clone(),
                rx_gui.clone(),
                rx_telemetry.clone(),
            )
        };

        #[cfg(target_os = "linux")]
        let tabula = Rc::new(RefCell::new(TabulaView::new(EditorMode::Code(
            "rust".to_string(),
        ))));
        #[cfg(target_os = "linux")]
        let tabula_widget = tabula.borrow().widget();

        #[cfg(target_os = "linux")]
        let matrix_widget = create_matrix_view(tx_brain.clone(), &cwd);

        #[cfg(all(target_os = "linux", feature = "gnome"))]
        let view = {
            // 3. THE SIDEBAR (Left Pane)
            let left_toolbar = adw::ToolbarView::new();
            let left_header = adw::HeaderBar::builder()
                .show_end_title_buttons(false)
                .build();
            let left_tab_view = adw::TabView::new();
            let left_tab_bar = adw::TabBar::new();
            left_tab_bar.set_view(Some(&left_tab_view));

            left_tab_view.append(&matrix_widget);
            let left_page = left_tab_view.page(&matrix_widget);
            left_page.set_title("Matrix");

            left_toolbar.add_top_bar(&left_header);
            left_toolbar.add_top_bar(&left_tab_bar);
            left_toolbar.set_content(Some(&left_tab_view));

            // 4. THE WORKSPACE (Right Pane)
            let right_toolbar = adw::ToolbarView::new();
            let right_header = adw::HeaderBar::builder()
                .show_start_title_buttons(false)
                .build();
            right_header.set_title_widget(Some(&gtk4::Label::new(Some("Una"))));

            let right_tab_view = adw::TabView::new();
            let right_tab_bar = adw::TabBar::new();
            right_tab_bar.set_view(Some(&right_tab_view));

            right_tab_view.append(&tabula_widget);
            let right_page = right_tab_view.page(&tabula_widget);
            right_page.set_title("Editor");

            right_toolbar.add_top_bar(&right_header);
            right_toolbar.add_top_bar(&right_tab_bar);
            right_toolbar.set_content(Some(&right_tab_view));

            // 5. THE MASTER LAYOUT
            let main_paned = Paned::builder()
                .orientation(Orientation::Horizontal)
                .start_child(&left_toolbar)
                .end_child(&right_toolbar)
                .position(260)
                .resize_start_child(false)
                .shrink_start_child(false)
                .wide_handle(true)
                .build();
            main_paned.upcast::<gtk4::Widget>()
        };

        #[cfg(all(target_os = "linux", not(feature = "gnome")))]
        let view: NativeView = {
            // --- Pure GTK4 Fallback ---
            let title_box = Box::new(Orientation::Horizontal, 0);

            let left_header = HeaderBar::builder().show_title_buttons(false).build();
            let separator = Separator::new(Orientation::Vertical);
            let right_header = HeaderBar::builder().show_title_buttons(true).build();
            right_header.set_title_widget(Some(&gtk4::Label::new(Some("Una"))));
            right_header.set_hexpand(true);

            title_box.append(&left_header);
            title_box.append(&separator);
            title_box.append(&right_header);

            #[cfg(all(target_os = "linux", feature = "gtk"))]
            window.set_titlebar(Some(&title_box));

            // Inner Workspaces
            let left_stack = Stack::new();
            left_stack.set_vexpand(true);
            left_stack.set_transition_type(StackTransitionType::SlideLeftRight);
            left_stack.add_titled(&matrix_widget, Some("matrix"), "Matrix");
            let left_switcher = StackSwitcher::builder()
                .stack(&left_stack)
                .halign(gtk4::Align::Center)
                .hexpand(true)
                .build();
            let left_toolbar = Box::new(Orientation::Horizontal, 0);
            left_toolbar.add_css_class("toolbar");
            left_toolbar.append(&left_switcher);
            let left_vbox = Box::new(Orientation::Vertical, 0);
            left_vbox.append(&left_toolbar);
            left_vbox.append(&left_stack);

            let right_stack = Stack::new();
            right_stack.set_vexpand(true);
            right_stack.set_transition_type(StackTransitionType::SlideLeftRight);
            right_stack.add_titled(&tabula_widget, Some("tabula"), "Editor");
            let right_switcher = StackSwitcher::builder()
                .stack(&right_stack)
                .halign(gtk4::Align::Center)
                .hexpand(true)
                .build();
            let right_toolbar = Box::new(Orientation::Horizontal, 0);
            right_toolbar.add_css_class("toolbar");
            right_toolbar.append(&right_switcher);
            let right_vbox = Box::new(Orientation::Vertical, 0);
            right_vbox.append(&right_toolbar);
            right_vbox.append(&right_stack);

            let main_paned = Paned::builder()
                .orientation(Orientation::Horizontal)
                .start_child(&left_vbox)
                .end_child(&right_vbox)
                .position(260)
                .resize_start_child(false)
                .shrink_start_child(false)
                .wide_handle(true)
                .build();

            // The Synchronization Trick
            main_paned.connect_position_notify(move |p| {
                let pos = p.position();
                left_header.set_width_request(pos);
            });

            #[cfg(all(target_os = "linux", feature = "gtk"))]
            let ret = main_paned.upcast::<gtk4::Widget>();
            #[cfg(not(all(target_os = "linux", feature = "gtk")))]
            let ret = ();

            ret
        };

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let view: NativeView = ();

        // 6. WIRE THE REFLEX ARC (UI Receiver Loop)
        #[cfg(target_os = "linux")]
        let tabula_clone = tabula.clone();
        #[cfg(target_os = "linux")]
        glib::MainContext::default().spawn_local(async move {
            while let Ok(update) = rx_gui.recv().await {
                match update {
                    GuiUpdate::EditorLoad(path_str) => {
                        let path = PathBuf::from(path_str);
                        println!("[UNA UI] ⚡ Loading into Tabula: {:?}", path);
                        tabula_clone.borrow().load_file(&path);
                    }
                    _ => {}
                }
            }
        });

        view
    };

    // 7. Ignite Quartzite
    Backend::new(APP_ID, bootstrap).run();

    Ok(())
}
