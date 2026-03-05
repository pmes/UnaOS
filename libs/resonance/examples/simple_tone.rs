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

use resonance::{AudioEngine, create_test_graph};
use std::thread;
use std::time::Duration;

fn main() -> Result<(), anyhow::Error> {
    println!("Initializing Resonance Audio Engine...");

    // Create the test graph (Osc -> Gain)
    let graph = create_test_graph();

    // Start the engine
    // This moves the graph into the audio thread.
    let _engine = AudioEngine::new(graph)?;

    println!("Audio Engine started. Playing 440Hz tone...");
    println!("Press Ctrl+C to stop.");

    // Keep the main thread alive to let the audio stream run.
    // In a real app, this would be the main event loop.
    thread::sleep(Duration::from_secs(5));

    println!("Stopping audio...");
    Ok(())
}
