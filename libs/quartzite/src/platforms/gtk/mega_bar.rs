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
use gtk4::{Box, CssProvider, HeaderBar, Label, Orientation, Paned};

pub struct MegaBar;

impl MegaBar {
    pub fn build(
        window: &gtk4::ApplicationWindow,
        title: &str,
        status_widget: &gtk4::Widget,
        left_tabs: &gtk4::Widget,
        right_tabs: &gtk4::Widget,
        left_content: &gtk4::Widget,
        right_content: &gtk4::Widget,
        brain_icon: &gtk4::Image,
    ) -> gtk4::Widget {
        // 0. The Dark Mode Hard-Wire (Direct GNOME DBus Wiretap)
        if let Some(source) = gtk4::gio::SettingsSchemaSource::default() {
            if source.lookup("org.gnome.desktop.interface", true).is_some() {
                let settings = gtk4::gio::Settings::new("org.gnome.desktop.interface");

                // Initial Check
                if settings.string("color-scheme").as_str() == "prefer-dark" {
                    window.add_css_class("una-dark");
                    if let Some(gtk_settings) = gtk4::Settings::default() {
                        gtk_settings.set_gtk_application_prefer_dark_theme(true);
                    }
                }

                // Listen for GNOME Quick Settings changes
                let win_clone = window.clone();
                settings.connect_changed(Some("color-scheme"), move |s, _| {
                    if s.string("color-scheme").as_str() == "prefer-dark" {
                        win_clone.add_css_class("una-dark");
                        if let Some(gtk_settings) = gtk4::Settings::default() {
                            gtk_settings.set_gtk_application_prefer_dark_theme(true);
                        }
                    } else {
                        win_clone.remove_css_class("una-dark");
                        if let Some(gtk_settings) = gtk4::Settings::default() {
                            gtk_settings.set_gtk_application_prefer_dark_theme(false);
                        }
                    }
                });

                // CRITICAL FIX: Keep the wiretap alive by tying it to the Window's lifecycle.
                // The window will hold this closure (and the settings object) until it is destroyed.
                window.connect_unrealize(move |_| {
                    let _ = &settings;
                });
            }
        }
        // 1. Inject CSS
        let provider = CssProvider::new();
        provider.load_from_resource("/org/una/vein/style.css");

        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().expect("No display"),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        // 2. Build the Titlebar Paned
        let title_paned = Paned::new(Orientation::Horizontal);
        title_paned.set_position(260); // Match starting position
        title_paned.set_wide_handle(false); // Hide the handle

        // Left Side: Header + Tabs
        let left_title_vbox = Box::new(Orientation::Vertical, 0);
        left_title_vbox.add_css_class("builder-sidebar"); // EXTEND LEFT SHADING UPWARD
        left_title_vbox.add_css_class("title-vbox");
        let fallback_left_header = HeaderBar::builder().show_title_buttons(false).build();
        left_title_vbox.append(&fallback_left_header);
        left_title_vbox.append(left_tabs);

        // Right Side: Header + Status + Tabs
        let right_title_vbox = Box::new(Orientation::Vertical, 0);
        right_title_vbox.add_css_class("builder-view"); // EXTEND RIGHT SHADING UPWARD
        right_title_vbox.add_css_class("title-vbox");
        right_title_vbox.set_hexpand(true);
        let fallback_right_header = HeaderBar::builder().show_title_buttons(true).build();
        fallback_right_header.set_title_widget(Some(&Label::new(Some(title))));

        // Pack the status widget and brain icon into the right header
        fallback_right_header.pack_start(status_widget);
        fallback_right_header.pack_start(brain_icon);

        // CRITICAL ALIGNMENT FIX FOR GTK:
        let header_size_group = gtk4::SizeGroup::new(gtk4::SizeGroupMode::Vertical);
        header_size_group.add_widget(&fallback_left_header);
        header_size_group.add_widget(&fallback_right_header);

        right_title_vbox.append(&fallback_right_header);
        right_title_vbox.append(right_tabs);

        title_paned.set_start_child(Some(&left_title_vbox));
        title_paned.set_end_child(Some(&right_title_vbox));

        window.set_titlebar(Some(&title_paned));

        // 3. Build the Content Frame
        let main_h_paned = Paned::new(Orientation::Horizontal);
        main_h_paned.set_position(260); // Slightly wider sidebar for TeleHUD
        main_h_paned.set_hexpand(true);
        main_h_paned.set_vexpand(true);
        main_h_paned.set_wide_handle(false);
        main_h_paned.set_shrink_start_child(false);
        main_h_paned.set_shrink_end_child(false);
        main_h_paned.set_resize_start_child(false);

        let left_vbox = Box::new(Orientation::Vertical, 0);
        left_vbox.add_css_class("builder-sidebar");
        left_vbox.set_size_request(260, -1);
        left_vbox.append(left_content);

        let right_vbox = Box::new(Orientation::Vertical, 0);
        right_vbox.add_css_class("builder-view");
        right_vbox.set_hexpand(true);
        right_vbox.append(right_content);

        main_h_paned.set_start_child(Some(&left_vbox));
        main_h_paned.set_end_child(Some(&right_vbox));

        // 4. Bind the Sync
        // CRITICAL FIX: Bidirectional binding ensures neither side can violate
        // the other's minimum width limits. They move in absolute lockstep.
        main_h_paned
            .bind_property("position", &title_paned, "position")
            .flags(glib::BindingFlags::SYNC_CREATE | glib::BindingFlags::BIDIRECTIONAL)
            .build();

        // 5. Return the Frame
        main_h_paned.upcast::<gtk4::Widget>()
    }
}
