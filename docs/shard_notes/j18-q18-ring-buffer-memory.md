<!--
SPDX-License-Identifier: LGPL-3.0-or-later
Copyright (C) 2026 The Architect & Una

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU Lesser General Public License as published by
the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.
-->

## 2026-03-19 - J18 "Ouroboros" 🐍 :: Ring Buffer Memory Refactoring

**Anomaly:** The `AppState` was maintaining historical chat logs and system console outputs using unbounded `Vec` arrays. During prolonged operating sessions (especially when processing streams of telemetry or chat messages), this architecture invariably led to unbound memory growth. Furthermore, relying on absolute array lengths (e.g., `history_cursor`) within the `translator.rs` bridge meant that truncating the array to reclaim memory would mathematically destroy the UI synchronization, as a smaller array size falsely indicated a "rollback" or "clear" state to the presentation layer.

**Resolution:** Replaced the unbounded `Vec` data structures in `libs/bandy/src/state.rs` with `std::collections::VecDeque`. We introduced a hard cap on retained memory defined by `MAX_STATE_CAPACITY` (1000 items). Rather than tracking the absolute length of the arrays, `translator.rs` now synchronizes using strictly monotonic sequence IDs (`history_seq` and `console_seq`). Whenever the backend pushes new items, it increments the sequence counters. The `translator.rs` bridge computes mathematical deltas against these sequence counters, safely extracting new trailing elements regardless of how many items were truncated from the head of the ring buffer. This enforces O(1) memory caps while ensuring precise UI synchronicity.