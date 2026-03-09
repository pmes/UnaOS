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