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
/// Uses a broadcast channel so multiple lobes (UI, Subconscious, AI)
/// can react to the same stimulus simultaneously.
#[derive(Clone)]
pub struct Synapse {
    tx: broadcast::Sender<SMessage>,
}

impl Synapse {
    pub fn new() -> Self {
        // 1024 action potentials in flight. If we hit this, the system is seizing.
        let (tx, _) = broadcast::channel(1024);
        Self { tx }
    }

    /// Fires a stimulus across the nervous system.
    pub fn fire(&self, msg: SMessage) {
        // We ignore SendError. If a tree falls in the forest...
        let _ = self.tx.send(msg);
    }

    /// Direct access to the transmitter.
    pub fn tx(&self) -> broadcast::Sender<SMessage> {
        self.tx.clone()
    }

    /// Sprout a new nerve ending to listen to the system.
    pub fn rx(&self) -> broadcast::Receiver<SMessage> {
        self.tx.subscribe()
    }
}

impl Default for Synapse {
    fn default() -> Self {
        Self::new()
    }
}
