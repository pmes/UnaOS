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

use crate::Event;
use gtk4::prelude::*;
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast::Receiver as BroadcastReceiver;

use bandy::state::AppState;
use bandy::SMessage;

pub struct CommsSpline {}

impl CommsSpline {
    pub fn new() -> Self {
        Self {}
    }

    pub fn bootstrap(
        &self,
        window: &crate::NativeWindow,
        tx_event: async_channel::Sender<Event>,
        app_state: Arc<RwLock<AppState>>,
        rx_synapse: BroadcastReceiver<SMessage>,
    ) -> crate::NativeView {
        #[cfg(feature = "gnome")]
        return build_gnome_ui(window, tx_event, app_state, rx_synapse);

        #[cfg(not(feature = "gnome"))]
        return build_gtk_ui(window, tx_event, app_state, rx_synapse);
    }
}

#[cfg(feature = "gnome")]
fn build_gnome_ui(
    window: &crate::NativeWindow,
    tx_event: async_channel::Sender<Event>,
    app_state: Arc<RwLock<AppState>>,
    rx_synapse: BroadcastReceiver<SMessage>,
) -> crate::NativeView {
    let brain_icon = gtk4::Image::from_icon_name("brain-symbolic");

    let workspace_widgets = crate::platforms::gtk::workspace::build(
        window,
        tx_event,
        app_state,
        rx_synapse,
        brain_icon.clone(),
    );

    // Assemble the GNOME specific TabView for the workspace
    let right_tab_view = libadwaita::TabView::new();
    let right_tab_bar = libadwaita::TabBar::new();
    right_tab_bar.set_view(Some(&right_tab_view));

    right_tab_view.append(&workspace_widgets.right_stack);
    let comms_page_ref = right_tab_view.page(&workspace_widgets.right_stack);
    comms_page_ref.set_title("Comms");

    crate::platforms::gnome::mega_bar::MegaBar::build(
        window.upcast_ref::<gtk4::ApplicationWindow>(),
        "",
        workspace_widgets.status_group.upcast_ref::<gtk4::Widget>(),
        workspace_widgets.left_switcher.upcast_ref::<gtk4::Widget>(),
        right_tab_bar.upcast_ref::<gtk4::Widget>(),
        workspace_widgets.left_stack.upcast_ref::<gtk4::Widget>(),
        right_tab_view.upcast_ref::<gtk4::Widget>(),
        &brain_icon,
    )
}

#[cfg(not(feature = "gnome"))]
fn build_gtk_ui(
    window: &crate::NativeWindow,
    tx_event: async_channel::Sender<Event>,
    app_state: Arc<RwLock<AppState>>,
    rx_synapse: BroadcastReceiver<SMessage>,
) -> crate::NativeView {
    let brain_icon = gtk4::Image::from_icon_name("brain-symbolic");

    let workspace_widgets = crate::platforms::gtk::workspace::build(
        window,
        tx_event,
        app_state,
        rx_synapse,
        brain_icon.clone(),
    );

    crate::platforms::gtk::mega_bar::MegaBar::build(
        window.upcast_ref::<gtk4::ApplicationWindow>(),
        "",
        workspace_widgets.status_group.upcast_ref::<gtk4::Widget>(),
        workspace_widgets.left_switcher.upcast_ref::<gtk4::Widget>(),
        workspace_widgets.right_switcher.upcast_ref::<gtk4::Widget>(),
        workspace_widgets.left_stack.upcast_ref::<gtk4::Widget>(),
        workspace_widgets.right_stack.upcast_ref::<gtk4::Widget>(),
        &brain_icon,
    )
}
