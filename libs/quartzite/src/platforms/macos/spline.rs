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

use std::sync::{Arc, RwLock, Mutex};

use objc2::rc::{Allocated, Retained};
use objc2_app_kit::{NSView, NSWindow, NSSplitViewController};

use async_channel;
use tokio::sync::broadcast::Receiver as BroadcastReceiver;

use bandy::state::AppState;
use bandy::SMessage;
use bandy::state::WorkspaceState;
use crate::Event;

use super::workspace::build_workspace;
use super::toolbar::build_toolbar;
use super::toolbar::ToolbarDelegate;

/// `MacOSSpline` is the exact equivalent to GTK's `CommsSpline`.
/// It is the architectural boundary where platform-agnostic `SMessage`
/// events from `bandy` are translated into native `AppKit` commands.
pub struct MacOSSpline {
    // Keep alive references to our strong objects so they don't drop
    // and cause dangling pointers in AppKit delegates.
    toolbar_delegate: Mutex<Option<Retained<ToolbarDelegate>>>,
    split_controller: Mutex<Option<Retained<NSSplitViewController>>>,
}

impl MacOSSpline {
    pub fn new() -> Self {
        Self {
            toolbar_delegate: Mutex::new(None),
            split_controller: Mutex::new(None),
        }
    }

    /// The single entry point to build the UI hierarchy on macOS.
    /// This is invoked inside `applicationDidFinishLaunching:`
    pub fn bootstrap(
        &self,
        window: &NSWindow,
        tx_event: async_channel::Sender<Event>,
        app_state: Arc<RwLock<AppState>>,
        rx_synapse: BroadcastReceiver<SMessage>,
        workspace_tetra: &WorkspaceState,
    ) -> Retained<NSView> {

        // 1. Build the Workspace Layout (Sidebar + Comms Pane)
        let (root_view, split_controller) = build_workspace(
            window,
            tx_event.clone(),
            app_state.clone(),
            rx_synapse,
            workspace_tetra,
        );
        if let Ok(mut sc) = self.split_controller.lock() {
            *sc = Some(split_controller);
        }

        // 2. Build and attach the Toolbar
        let toolbar_delegate = build_toolbar(window, tx_event);
        if let Ok(mut td) = self.toolbar_delegate.lock() {
            *td = Some(toolbar_delegate);
        }

        // 3. We return `root_view` to set as `contentView`.

        root_view
    }
}
