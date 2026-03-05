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

use crate::cortex::Cortex;
use bandy::{SMessage, Synapse};
use std::path::PathBuf;
use tokio::sync::broadcast::error::RecvError;

/// The Autonomous Loop.
/// Runs silently in the background, absorbing the nervous system's telemetry.
pub async fn ignite(vault_path: PathBuf, mut synapse: Synapse) {
    let mut cortex = Cortex::awaken(vault_path, &mut synapse);
    let mut rx = synapse.rx();

    log::info!(">> [LUMEN CORE] Synapses firing. Autonomous loop engaged.");

    loop {
        match rx.recv().await {
            Ok(msg) => match msg {
                SMessage::UserPrompt(text) => {
                    cortex.imprint("stimulus.prompt", text.as_bytes());
                }
                SMessage::FileEvent { path, event } => {
                    let payload = format!("{}|{}", path, event);
                    cortex.imprint("stimulus.fs", payload.as_bytes());
                }
                SMessage::Kill(target) if target == "lumen" => {
                    log::warn!(">> [LUMEN CORE] Kill signal received. Severing.");
                    break;
                }
                SMessage::Log {
                    source, content, ..
                } => {
                    let payload = format!("{}: {}", source, content);
                    cortex.imprint("stimulus.log", payload.as_bytes());
                }
                _ => {} // The subconscious absorbs the noise.
            },
            Err(RecvError::Lagged(skipped)) => {
                log::warn!(
                    ">> [LUMEN CORE] Synapse overloaded. Skipped {} stimuli.",
                    skipped
                );
            }
            Err(RecvError::Closed) => {
                log::warn!(">> [LUMEN CORE] Nervous system severed. Shutting down.");
                break;
            }
        }
    }
}
