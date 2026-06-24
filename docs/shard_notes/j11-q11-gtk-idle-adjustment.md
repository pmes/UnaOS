<!--
  SPDX-License-Identifier: LGPL-3.0-or-later
  Copyright (C) 2026 The Architect & Una
-->

## Deferring GTK Adjustments for Lumen Auto-Scroll
**Anomaly:** In the GTK4 Lumen UI, bulk updates injected via `console_store.splice()` in `reactor.rs` triggered `gtk_adjustment_configure` assertion failures (`lower + page_size <= upper`). The view's scroll adjustment `upper_notify` callback was firing and attempting to execute `.set_value()` synchronously, before the view's layout engine had completed allocating geometry for the newly appended items, leading to mathematically invalid bounds during the scroll calculation.
**Resolution:** Wrapped the `vadjustment.set_value()` logic within a `gtk4::glib::idle_add_local` closure inside the `upper_notify` connection in `comms.rs`. This correctly defers the adjustment value updates to the GTK idle loop, yielding to the GTK frame cycle. By waiting for the view's layout pass to complete, `page_size` and `upper` are mathematically finalized and accurate when the scroll bounds are configured, maintaining rapid batched rendering while resolving the assertion panic.
## Boot-time State Priming for ScrolledWindow Geometry
**Anomaly:** At boot time, the GTK4 `ScrolledWindow` generated a `gtk_adjustment_configure` assertion (`lower + page_size <= upper`). Because the underlying `console_store` was empty at launch, the `ListView` shrank to a 0-pixel height. When `ScrolledWindow` attempted to configure scrollbar math against a 0-height geometry (`upper = 0`), the calculation `0 + page_size <= 0` mathematically failed, crashing the layout constraints internally before any data could be added.
**Resolution:** Implemented "State Priming" in `comms.rs`. Instantiated a single `HistoryObject` representing the system boot state ("UnaOS Telemetry Link Established") and immediately appended it to `console_store` upon initialization. This mathematically guarantees the `ListView` Maps to the screen with a valid height geometry > 0, entirely preventing the assertion. This preserves the internal `GtkScrollable` interface and layout O(N) performance, overriding previous hack-based protocols that broke the model via nested layout wrappers.
