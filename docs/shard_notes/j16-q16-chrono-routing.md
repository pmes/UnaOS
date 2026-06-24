<!--
  SPDX-License-Identifier: GPL-3.0-or-later
  Copyright (C) 2026 The Architect & Una

  This file is part of UnaOS.

  UnaOS is free software: you can redistribute it and/or modify
  it under the terms of the GNU General Public License as published by
  the Free Software Foundation, either version 3 of the License, or
  (at your option) any later version.

  UnaOS is distributed in the hope that it will be useful,
  but WITHOUT ANY WARRANTY; without even the implied warranty of
  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
  GNU General Public License for more details.

  You should have received a copy of the GNU General Public License
  along with UnaOS.  If not, see <https://www.gnu.org/licenses/>.
-->

## 2026-03-19 - J16 "Chronos" ⏳ :: Chronological Inversion Routing Fix

**Anomaly:** `reactor.rs` was indiscriminately splicing all history updates at index 0 (`HistoryBatch`), causing live chat and new items to render at the top (chronological inversion) instead of correctly appending to the bottom.

**Resolution:** Split the GTK presentation pipeline (`GuiUpdate`) into two separate, mathematically specific pathways:
1. `HistorySeed`: Used strictly when the history cursor is 0. Continues to prepend via `splice(0, 0, &batch)` to retain the `boot_obj` geometry buffer.
2. `HistoryAppend`: Used for all subsequent updates. Correctly appends to the bottom via `let len = pointers.console_store.n_items(); pointers.console_store.splice(len, 0, &batch);`.
This explicitly maintains the physics of causality without destroying pre-mapped UI components.
