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
use async_channel;
use std::sync::{Arc, Mutex};

/// The connective tissue of the nervous system.
/// Uses bounded async channels to broadcast to multiple lobes (UI, Subconscious, AI)
/// so they can react to the same stimulus simultaneously while enforcing strict backpressure.
#[derive(Clone)]
pub struct Synapse {
    txs: Arc<Mutex<Vec<async_channel::Sender<SMessage>>>>,
}

impl Synapse {
    pub fn new() -> Self {
        Self {
            txs: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Fires a stimulus across the nervous system synchronously with strict backpressure.
    pub fn fire(&self, msg: SMessage) {
        let txs = self.txs.lock().unwrap().clone();
        for tx in txs {
            // We block to enforce backpressure. If a tree falls in the forest...
            let _ = tx.send_blocking(msg.clone());
        }
    }

    /// Fires a stimulus asynchronously with strict backpressure.
    pub async fn fire_async(&self, msg: SMessage) {
        let txs = self.txs.lock().unwrap().clone();
        for tx in txs {
            let _ = tx.send(msg.clone()).await;
        }
    }

    /// Sprout a new nerve ending to listen to the system.
    pub fn rx(&self) -> async_channel::Receiver<SMessage> {
        // 1024 action potentials in flight. If we hit this, the system is strictly backpressured.
        let (tx, rx) = async_channel::bounded(1024);
        self.txs.lock().unwrap().push(tx);
        rx
    }
}

impl Default for Synapse {
    fn default() -> Self {
        Self::new()
    }
}
