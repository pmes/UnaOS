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
use gtk4::{Box, CssProvider, Orientation, Paned};
use libadwaita as adw;

pub struct MegaBar;

impl MegaBar {
    pub fn build(
        window: &gtk4::ApplicationWindow,
        _title: &str, // Native GNOME handles titles via the window system or we can let it be
        status_widget: &gtk4::Widget,
        left_tabs: &gtk4::Widget,
        right_tabs: &gtk4::Widget,
        left_content: &gtk4::Widget,
        right_content: &gtk4::Widget,
        brain_icon: &gtk4::Image,
        workspace_tetra: &bandy::state::WorkspaceState,
    ) -> gtk4::Widget {
        // 0. The Dark Mode Hard-Wire (GNOME)
        let style_manager = adw::StyleManager::default();
        let win_clone = window.clone();
        if style_manager.is_dark() {
            win_clone.add_css_class("una-dark");
        }
        style_manager.connect_dark_notify(move |sm| {
            if sm.is_dark() {
                win_clone.add_css_class("una-dark");
            } else {
                win_clone.remove_css_class("una-dark");
            }
        });

        // 1. Inject CSS
        let provider = CssProvider::new();
        provider.load_from_resource("/org/una/vein/style.css");

        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().expect("No display"),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        // 2. Build Content Frames with ToolbarViews
        let main_h_paned = Paned::new(Orientation::Horizontal);

        // Calculate the initial split position based on the proportional ratio.
        // If default_width is not set or returns 0 (some layouts), fallback to 1024.
        let default_width = window.default_width();
        let reference_width = if default_width > 0 { default_width as f32 } else { 1024.0 };
        let initial_position = (reference_width * workspace_tetra.split_ratio) as i32;

        main_h_paned.set_position(initial_position);
        main_h_paned.set_hexpand(true);
        main_h_paned.set_vexpand(true);
        main_h_paned.set_wide_handle(false);
        main_h_paned.set_shrink_start_child(false);
        main_h_paned.set_shrink_end_child(false);
        main_h_paned.set_resize_start_child(false);

        // --- LEFT SIDE ---
        let left_toolbar = adw::ToolbarView::new();
        left_toolbar.set_widget_name("left");
        left_toolbar.add_css_class("builder-sidebar");
        // Maintain a minimum size instead of a hardcoded request to allow the paned to dictate width.
        left_toolbar.set_size_request((1024.0 * 0.15) as i32, -1); // Safe minimum fallback

        // Strip native drop-shadows
        left_toolbar.set_top_bar_style(adw::ToolbarStyle::Flat);

        let left_header = adw::HeaderBar::builder()
            .show_end_title_buttons(false)
            .build();

        left_toolbar.add_top_bar(&left_header);
        left_toolbar.add_top_bar(left_tabs);

        // Wrap the content so we can apply the class reliably if needed,
        // though ToolbarView with the class handles the background.
        let left_vbox = Box::new(Orientation::Vertical, 0);
        left_vbox.append(left_content);
        left_toolbar.set_content(Some(&left_vbox));

        // --- RIGHT SIDE ---
        let right_toolbar = adw::ToolbarView::new();
        right_toolbar.set_widget_name("right");
        right_toolbar.add_css_class("builder-view");
        right_toolbar.set_hexpand(true);

        // Strip native drop-shadows
        right_toolbar.set_top_bar_style(adw::ToolbarStyle::Flat);

        let right_header = adw::HeaderBar::builder()
            .show_start_title_buttons(false) // Only show the window controls on the far right
            .build();

        // Pack the status widget and brain icon
        right_header.pack_start(status_widget);
        right_header.pack_start(brain_icon);

        right_toolbar.add_top_bar(&right_header);
        right_toolbar.add_top_bar(right_tabs);

        let right_vbox = Box::new(Orientation::Vertical, 0);
        right_vbox.append(right_content);
        right_toolbar.set_content(Some(&right_vbox));

        // --- ALIGNMENT FIX FOR GNOME TABS ---
        let tab_size_group = gtk4::SizeGroup::new(gtk4::SizeGroupMode::Vertical);
        tab_size_group.add_widget(left_tabs);
        tab_size_group.add_widget(right_tabs);

        main_h_paned.set_start_child(Some(&left_toolbar));
        main_h_paned.set_end_child(Some(&right_toolbar));

        main_h_paned.upcast::<gtk4::Widget>()
    }
}
