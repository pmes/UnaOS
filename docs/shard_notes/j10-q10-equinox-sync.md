<!--
  SPDX-License-Identifier: LGPL-3.0-or-later
  Copyright (C) 2026 The Architect & Una

  This file is part of UnaOS.

  UnaOS is free software: you can redistribute it and/or modify
  it under the terms of the GNU Lesser General Public License as published by
  the Free Software Foundation, either version 3 of the License, or
  (at your option) any later version.

  UnaOS is distributed in the hope that it will be useful,
  but WITHOUT ANY WARRANTY; without even the implied warranty of
  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
  GNU Lesser General Public License for more details.

  You should have received a copy of the GNU Lesser General Public License
  along with UnaOS.  If not, see <https://www.gnu.org/licenses/>.
-->

## 2026-10-27 - Equinox Synchronization Perfection

**Anomaly:**
The data synchronization loops in `reactor.rs` and `translator.rs` were locking the GTK4 rendering thread under heavy load. Specifically, `translator.rs` was cloning the entire `state.history` array on every `StateInvalidated` ping, causing an O(N) memory allocation bottleneck. Additionally, `reactor.rs` was using a looping `.append()` method for sequential `ConsoleLog` items, which forced the `ListView` geometry to recalculate on each single item and created severe UI stuttering during massive system-log or memory-dump operations.

**Resolution:**
The architecture was overhauled to ensure mathematical zero-stutter scaling.

1. **The Translator Throttle:** The cursor logic was relocated entirely to `translator.rs`. `history_sync_cursor` and `console_log_cursor` were introduced. `translator.rs` now extracts only the delta slice (e.g. `st.history[cursor..]`), and only clones and fires the strictly new data over the `tx_gui` async channel. If a length drop is detected (indicating a state rollback/clear), the cursors reset to `0`, emit a `ClearConsole` ping, and rebuild the state automatically.
2. **The Reactor Splice:** `reactor.rs` was stripped of its cursors, becoming a pure consumer. Both `GuiUpdate::HistoryBatch` and the upgraded `GuiUpdate::ConsoleLogBatch` now map raw data payloads into generic `HistoryObject` items, bundle them into standard Vec batches, and perform a single `console_store.splice(len, 0, &batch)`. This GTK-native batch injection bypasses repeated rendering recalculations and maintains absolute UI fluidity under massive load.
