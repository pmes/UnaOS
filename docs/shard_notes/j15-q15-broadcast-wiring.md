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
