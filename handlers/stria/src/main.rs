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

mod engine;
mod ui;

use gneiss_pal::{KeyCode, WaylandApp, WindowEvent};

fn main() {
    println!(":: Stria Media System v0.1 ::");

    // 1. Ignite the Engine (The Stevedore)
    let cores = num_cpus::get();
    println!(":: ENGINE ONLINE ({} Cores Ready) ::", cores);

    // 2. Launch the Interface (The Gneiss PAL)
    println!(":: LINKING GNEISS PAL... ::");
    let mut app = WaylandApp::new().expect("Failed to initialize Wayland connection");

    // 3. Open the Viewport
    let _window = app
        .create_window(1280, 720, "Stria [Engine: Idle]")
        .expect("Failed to create window");

    println!(":: SYSTEM ONLINE. PRESS ESC TO SHUTDOWN. ::");

    // 4. Enter the Event Loop
    app.run(move |event| match event {
        WindowEvent::CloseRequested => {
            println!(":: SHUTDOWN SEQUENCE ::");
            std::process::exit(0);
        }
        WindowEvent::KeyboardInput {
            key: KeyCode::Escape,
            ..
        } => {
            println!(":: EMERGENCY STOP ::");
            std::process::exit(0);
        }
        _ => {}
    });
}
