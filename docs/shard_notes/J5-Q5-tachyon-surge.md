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

## 2026-10-27 - [The Deterministic Shutdown & Synaptic Backpressure]

**Anomaly:** The system was bleeding threads in `trigger_upload` by ignoring the Tokio worker pool and spawning independent threads and runtimes. The Bandy synapse was dropping high-frequency telemetry via a lossy `tokio::sync::broadcast` channel, causing `RecvError::Lagged` panics. Finally, the main shutdown sequence relied on blind `.abort()` and `std::thread::sleep(...)`, risking dirty UnaFS flushes.

**Resolution:**
1. Re-routed `trigger_upload` inside `VeinHandler` to directly use `tokio::spawn`, keeping all workloads firmly within the single Tokio reactor.
2. Upgraded the `Synapse` channel from a lossy broadcast to a secure `async_channel::bounded(1024)`. This strictly enforces MPMC backpressure across the system, guaranteeing zero internal messages are dropped.
3. Intercepted the OS termination signal natively inside `core::ignite` via a `tokio::select!`. The main thread now explicitly awaits the `core_handle` instead of aborting it, mathematically verifying the synchronous UnaFS flush before process termination.
