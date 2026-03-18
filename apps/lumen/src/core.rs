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

/// The Autonomous Loop.
/// Runs silently in the background, absorbing the nervous system's telemetry.
pub async fn ignite(
    vault_path: PathBuf,
    mut synapse: Synapse,
    mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
) {
    let mut cortex = Cortex::awaken(vault_path, &mut synapse);
    let mut rx = synapse.subscribe();

    log::info!(">> [LUMEN CORE] Synapses firing. Autonomous loop engaged.");

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                log::info!(">> [LUMEN CORE] Termination broadcast caught. Flushing cortex and severing.");
                break;
            }
            res = rx.recv() => match res {
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
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    log::warn!(">> [LUMEN CORE] Receiver lagged, dropping missed events.");
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    log::warn!(">> [LUMEN CORE] Nervous system severed. Shutting down.");
                    break;
                }
            }
        }
    }
}
