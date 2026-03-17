<!--
  SPDX-License-Identifier: LGPL-3.0-or-later
  Copyright (C) 2026 The Architect & Una
-->

## 2026-10-27 - Deferring GTK Adjustments for Lumen Auto-Scroll
**Anomaly:** In the GTK4 Lumen UI, bulk updates injected via `console_store.splice()` in `reactor.rs` triggered `gtk_adjustment_configure` assertion failures (`lower + page_size <= upper`). The view's scroll adjustment `upper_notify` callback was firing and attempting to execute `.set_value()` synchronously, before the view's layout engine had completed allocating geometry for the newly appended items, leading to mathematically invalid bounds during the scroll calculation.
**Resolution:** Wrapped the `vadjustment.set_value()` logic within a `gtk4::glib::idle_add_local` closure inside the `upper_notify` connection in `comms.rs`. This correctly defers the adjustment value updates to the GTK idle loop, yielding to the GTK frame cycle. By waiting for the view's layout pass to complete, `page_size` and `upper` are mathematically finalized and accurate when the scroll bounds are configured, maintaining rapid batched rendering while resolving the assertion panic.
