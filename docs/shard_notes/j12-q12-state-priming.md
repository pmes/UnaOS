<!--
  SPDX-License-Identifier: LGPL-3.0-or-later
  Copyright (C) 2026 The Architect & Una
-->

## 2026-10-27 - Boot-time State Priming for ScrolledWindow Geometry
**Anomaly:** At boot time, the GTK4 `ScrolledWindow` generated a `gtk_adjustment_configure` assertion (`lower + page_size <= upper`). Because the underlying `console_store` was empty at launch, the `ListView` shrank to a 0-pixel height. When `ScrolledWindow` attempted to configure scrollbar math against a 0-height geometry (`upper = 0`), the calculation `0 + page_size <= 0` mathematically failed, crashing the layout constraints internally before any data could be added.
**Resolution:** Implemented "State Priming" in `comms.rs`. Instantiated a single `HistoryObject` representing the system boot state ("UnaOS Telemetry Link Established") and immediately appended it to `console_store` upon initialization. This mathematically guarantees the `ListView` Maps to the screen with a valid height geometry > 0, entirely preventing the assertion. This preserves the internal `GtkScrollable` interface and layout O(N) performance, overriding previous hack-based protocols that broke the model via nested layout wrappers.