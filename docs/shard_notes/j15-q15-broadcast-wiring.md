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

## 2026-03-18 - UI Boundary Broadcast Migration
**Anomaly:** The `quartzite` workspace initialization (via `translator::spawn_translator`) and `app.rs`/`spline.rs` signatures were holding onto deprecated `async_channel::Receiver<SMessage>` expectations. This violated the zero-copy broadcast pattern required for `Synapse`, causing mismatched type compiler errors (`E0308`) when trying to pass the new `tokio::sync::broadcast::Receiver`. MPMC channels load-balance messages, which starves the UI if a background listener wins the race.

**Resolution:** Upgraded all `rx_synapse` type signatures in `libs/quartzite/src/platforms/gtk/workspace/mod.rs`, `libs/quartzite/src/platforms/gtk/spline.rs`, and `libs/quartzite/src/spline.rs` to `tokio::sync::broadcast::Receiver<SMessage>`. Documented the architectural shift to "pub/sub physics" via comments. Finally, patched `apps/lumen/src/main.rs` to directly call `synapse.subscribe()` and drop the deprecated `async_channel` implementation during boot-up.

## Next Steps / Pending Architecture Refinements
Based on previous deep scans of the Shard Notes (`j8-q8` through `j14-q14`), the following incomplete functionalities and required structural refinements must be addressed by subsequent Shards:

1. **Chat Bubble Rendering Desync (J9 Flux Wiring Issue):**
   * While system logs function, the UI still refuses to draw historical chat bubbles. There is a confirmed desync between the frontend list store memory allocations and the backend `AppState`.
   * **Action:** Examine `translator.rs` and the `reactor.rs` loop. Ensure `AppState.history` is properly translated across the `GuiUpdate::HistoryBatch` channel.
   * **Hypothesis:** Historical bubbles might be dropping because they arrive during the blocked `ScrolledWindow` shadow-mapping pass. Additionally, the `glib::idle_add_local` scroll adjustment delay inside `comms.rs` might be inadvertently masking or blocking the render of the `HistoryBatch` splice by consuming the UI thread cycle. GTK may discard the visual nodes if the batch splice occurs but the idle loop fails or blocks layout reallocation.

2. **Broadcasting & Half-Assed MPMC Cleanup (J14 / J15 Polish):**
   * The transition from `async_channel` to `tokio::sync::broadcast` is functionally complete at the boundaries, but the internal mechanisms for handling `tokio::sync::broadcast::error::RecvError::Lagged` require review.
   * **Action:** Ensure we are correctly and gracefully dropping missed transient pings without building complex re-sync logic, relying purely on `AppState` as the single source of truth. Confirm no residual MPMC listeners exist that could cause race conditions.

3. **GTK List Rendering Stability (J11 / J13 Polish):**
   * Ensure that the "State Priming" boot state (instantiating a 1-pixel height `upper` boundary) inside `comms.rs` and the scroll math quarantine (`lower + page_size <= upper`) are holding up perfectly under the new broadcast physics when rapid `HistoryBatch` updates arrive.
