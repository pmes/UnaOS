<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright (C) 2026 The Architect & Una

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU General Public License for more details.

You should have received a copy of the GNU General Public License
along with this program.  If not, see <https://www.gnu.org/licenses/>.
-->

## 2024-05-18 - [J3-Q3 Bismuth Fluid Glass Integration]

**Anomaly:** Routing raw payloads directly from the C++ engine to QML risks blocking the Tokio runtime during heavy transmission intervals, causing potential UI jitter. Moreover, modifying the core IPC `SMessage` to echo the payload back introduces unnecessary structural overhead and violates the architectural mandate to avoid touching the core IPC enums.

**Resolution:** Leveraged CXX-Qt 0.8 `#[qsignal]` to broadcast the raw payload directly from the `vein_bridge` `send_message` closure across the FFI boundary immediately *before* it is committed to the wire. Built a dedicated `NetworkLogModelRust` that consumes this signal in Qt space via QML `Connections`, caching the data strictly on the heap for zero-copy visual rendering. This provides a transparent truth view without polluting the autonomic routing system or mutating `SMessage`. Re-engineered `NexusChat.qml` logic to calculate layout fluid geometry mathematically using `Math.min()` bounded bounds, removing any need for legacy Qt layouts.

## 2024-05-18 - [J3-Q3 Pre-Flight Geometry and Primitive Restorations]

**Anomaly:** Initial UI configurations exhibited deep architectural logic flaws (e.g. binding loops preventing user edit capabilities inside the pre-flight overlay, Qt 5 MessageDialog legacy crashes preventing window rendering, and multiple instantiated bridges generating a CXX-Qt `OnceLock` fracture).

**Resolution:** Repaired `NexusChat.qml` by eradicating multiple `VeinBridge` invocations in favor of routing a global generic `property var backend`. Fixed pre-flight bindings by utilizing dynamic `property alias` parameters and initializing string overrides strictly on the event loop closure `onPayloadReadyForReview`. Successfully integrated `QtQuick.Controls` Dialog (modal overlay centering perfectly within the application layout to prevent Wayland pop-out behavior). Implemented Can-Am reset hack (`begin_reset_model()`) to safeguard model insertions. Enabled TextEdit UI hooks inside Truth View.

***

### 📝 RECOMMENDATIONS FOR J4 :: DEEP SCAN ANALYSIS

**Target Anomaly: `[WARNING] :: DIRTY MOUNT DETECTED. TORN TRANSACTION IN JOURNAL.`**

*   **Analysis:** The `unafs` crate logs this warning upon boot (`libs/unafs/src/fs.rs: mount()`) when the internal journal identifies an incomplete commit or torn transaction sequence across blocks during initialization.
*   **Investigation:**
    *   I confirmed that `UnaFS<D>` already explicitly implements the standard Rust `Drop` trait, correctly invoking `self.sync_metadata()` and `self.device.flush()` to ensure atomic saves during a graceful shutdown sequence.
    *   The occurrence of this warning in the console output indicates that the parent application (`lumen` / `vein` reactor) is encountering an ungraceful shutdown. This typically happens when the process panics, is externally killed via SIGKILL, or the Tokio runtime terminates before dropping its static/global variables correctly.
*   **J4 Strategic Action Plan:**
    1.  **Intercept Signal Handlers:** Implement explicit OS signal handlers (SIGINT, SIGTERM) inside the primary application binary (`apps/lumen/src/main.rs`). Instead of terminating abruptly, capture the interrupt and initiate a graceful "Drain & Drop" sequence, halting new network traffic and awaiting the `UnaFS` storage `Drop` execution.
    2.  **Audit the Storage Lock:** If `UnaFS` is wrapped inside a global static `RwLock` or `Mutex` (e.g. `OnceLock`), Rust's global shutdown sequence *does not* automatically execute `Drop` traits on static variables. J4 must explicitly acquire the lock and `.take()` or manually drop the file system instance during the termination sequence.
    3.  **Journal Recovery Expansion:** Although `journal.check_recovery()` detects the torn transaction, J4 should evaluate if the logic kernel successfully replays or discards the torn block. If it only prints the warning without executing the rollback, implement the standard WAL replay loop.