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

use crate::SMessage;
use tokio::sync::broadcast;

/// The connective tissue of the nervous system.
/// Uses a tokio broadcast channel to broadcast to multiple lobes (UI, Subconscious, AI)
/// so they can react to the same stimulus simultaneously.
#[derive(Clone)]
pub struct Synapse {
    tx: broadcast::Sender<SMessage>,
}

impl Synapse {
    pub fn new() -> Self {
        // 1024 action potentials in flight. Buffer depth prevents immediate lagging.
        let (tx, _rx) = broadcast::channel(1024);
        Self { tx }
    }

    /// Fires a stimulus across the nervous system synchronously.
    pub fn fire(&self, msg: SMessage) {
        // Broadcast ignores SendError (which happens if there are no active receivers)
        let _ = self.tx.send(msg);
    }

    /// Fires a stimulus asynchronously.
    pub async fn fire_async(&self, msg: SMessage) {
        // Broadcast ignores SendError
        let _ = self.tx.send(msg);
    }

    /// Sprout a new nerve ending to listen to the system.
    pub fn subscribe(&self) -> broadcast::Receiver<SMessage> {
        self.tx.subscribe()
    }
}

impl Default for Synapse {
    fn default() -> Self {
        Self::new()
    }
}
