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

use resonance::{AudioCommand, AudioEngine, create_test_graph};
use std::io::{self, Write};
use std::thread;
use std::time::Duration;

fn main() -> Result<(), anyhow::Error> {
    println!("Initializing Resonance Audio Engine (Interactive Mode)...");

    // Create the test graph (Osc -> Gain)
    let graph = create_test_graph();

    // Start the engine
    // We get back the engine (to keep stream alive) and the producer (to send commands)
    let (_engine, mut producer) = AudioEngine::new(graph)?;

    println!("Audio Engine started. Playing 440Hz tone.");
    println!("Commands:");
    println!("  <number> -> Set Frequency (e.g., 880)");
    println!("  stop     -> Exit");
    println!("---------------------------------------");

    // Spawn a thread to handle input to avoid blocking the main thread
    // (though main thread is just waiting here anyway, but good practice).
    // Actually, we can just run the input loop in main.

    let stdin = io::stdin();
    let mut input = String::new();

    loop {
        print!("> ");
        io::stdout().flush()?;
        input.clear();

        if stdin.read_line(&mut input)? == 0 {
            break; // EOF
        }

        let trimmed = input.trim();
        if trimmed.eq_ignore_ascii_case("stop") || trimmed.eq_ignore_ascii_case("exit") {
            // Send stop command just in case (though we exit process)
            let _ = producer.push(AudioCommand::Stop);
            break;
        }

        if let Ok(freq) = trimmed.parse::<f64>() {
            println!("Setting frequency to {:.2} Hz", freq);
            if producer
                .push(AudioCommand::SetMasterFrequency(freq))
                .is_err()
            {
                eprintln!("Command queue full!");
            }
        } else {
            println!("Invalid command. Enter a number or 'stop'.");
        }
    }

    println!("Stopping audio...");
    Ok(())
}
