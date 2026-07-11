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
use std::io::{self, Write};

fn main() -> Result<(), anyhow::Error> {
    println!("Initializing Resonance Audio Engine (Interactive Mode)...");

    // Create the test graph (Osc -> Gain; node 0 = osc, node 1 = gain)
    let graph = create_test_graph();

    // Start the engine.
    // We get back the engine (to keep the stream alive) and the handle
    // (to send commands).
    let (engine, mut handle) = AudioEngine::new(graph)?;

    println!(
        "Audio Engine started ({} Hz device). Playing 440Hz tone.",
        engine.sample_rate
    );
    println!("Commands:");
    println!("  <number>   -> Set frequency in Hz (e.g., 880)");
    println!("  g <number> -> Set gain (e.g., g 0.3)");
    println!("  stop       -> Silence the output");
    println!("  start      -> Resume the output");
    println!("  quit       -> Exit");
    println!("---------------------------------------");

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
        if trimmed.eq_ignore_ascii_case("quit") || trimmed.eq_ignore_ascii_case("exit") {
            break;
        }

        if trimmed.eq_ignore_ascii_case("stop") {
            println!("Silencing output (is_active -> false).");
            if !handle.stop() {
                eprintln!("Command queue full!");
            }
            continue;
        }

        if trimmed.eq_ignore_ascii_case("start") {
            println!("Resuming output.");
            handle.start();
            continue;
        }

        if let Some(gain_str) = trimmed
            .strip_prefix("g ")
            .or_else(|| trimmed.strip_prefix("G "))
        {
            if let Ok(gain) = gain_str.trim().parse::<f64>() {
                println!("Setting gain to {:.3}", gain);
                // Node 1 = the test graph's gain node, param 0 = base_gain.
                if !handle.set_param(1, 0, gain) {
                    eprintln!("Command queue full!");
                }
            } else {
                println!("Invalid gain. Try: g 0.3");
            }
            continue;
        }

        if let Ok(freq) = trimmed.parse::<f64>() {
            println!("Setting frequency to {:.2} Hz", freq);
            if !handle.set_frequency(freq) {
                eprintln!("Command queue full!");
            }
        } else {
            println!("Invalid command. Enter a number, 'g <gain>', 'stop', 'start', or 'quit'.");
        }
    }

    println!("Stopping audio...");
    Ok(())
}
