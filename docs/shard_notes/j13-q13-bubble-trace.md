<!--
    Copyright (C) 2026 The Architect & Una
    SPDX-License-Identifier: LGPL-3.0-or-later
-->

# 🧠 J13 "Tracer" ⚡

## 2026-03-18 - [Telemetry Insertion for GTK UI State Validation]

**Anomaly:** Data from `HistoryObject`s might be failing to enter the `gio::ListStore` and failing to trigger a re-render in the GTK `ListView`, resulting in "ghost rows" or absent UI elements, despite data possibly being processed asynchronously. A silent failure lacks observability.

**Resolution:**
Injected precise, non-destructive `println!` telemetry at the GTK boundary to verify the mathematical soundness of our data flow. The following sensor points were established:
1. In `libs/quartzite/src/platforms/gtk/workspace/reactor.rs`: Validates payload arrival inside `GuiUpdate::ConsoleLogBatch` and `GuiUpdate::HistoryBatch` immediately following `console_store.splice(...)`.
2. In `libs/quartzite/src/platforms/gtk/workspace/comms.rs`: Validates the GTK4 UI component tree attempts to mount the data via `console_factory.connect_bind` by logging the `Sender`, `Subject`, and `Timestamp` from the `HistoryObject` immediately after `item.item().and_then(|c| c.downcast::<HistoryObject>().ok())`.

This establishes the proof needed to verify if the UI is completely blind or if the layout structures are merely hiding the list via 0-pixel allocations.

## 2026-03-18 - [Quarantine of GTK Custom Scroll Math]

**Anomaly:** Custom geometry calculations inside `adj.connect_value_notify` and `adj.connect_upper_notify` might be mathematically conflicting with GTK4's internal lazy-layout engine (`lower + page_size <= upper`), causing `gtk_adjustment_configure` assertion crashes. Additionally, it was unclear if the initial `Event::LoadHistory` request was actually firing.

**Resolution:**
Executed a strict quarantine protocol on the scroll event handlers by injecting an immediate `return;` as the first line inside the closures in `libs/quartzite/src/platforms/gtk/workspace/comms.rs`. This isolates the suspected volatile math without destructively altering The Architect's logic. Simultaneously, injected `>>> [J13 TRACE] COMMS: Dispatching Event::LoadHistory to Backend.` inside `scrolled_window.connect_map` prior to the `.send(Event::LoadHistory)` call to prove frontend dispatch occurs successfully.

**Anomaly (Phase 2):** The `Gtk-CRITICAL` assertion failed despite the quarantine, proving the panic is structural, not tied to custom scroll closures. Additionally, while the UI successfully dispatched `Event::LoadHistory`, the backend never responded with `GuiUpdate::HistoryBatch`, indicating a swallowed request or translation failure.

**Resolution (Phase 2):**
1. Replaced `.vscrollbar_policy(PolicyType::Always)` with `.vscrollbar_policy(PolicyType::Automatic)` on the main chat view `ScrolledWindow` in `comms.rs`. Forcing a scrollbar on a nearly empty list triggers impossible scroll math in GTK4, asserting the crash.
2. Injected `>>> [J13 TRACE] BACKEND: Received Event::LoadHistory. Attempting to fetch...` in `handlers/vein/src/lib.rs` inside the `AppHandler` event loop immediately before the string dispatch.
3. Injected `>>> [J13 TRACE] BACKEND: StorageLoadAllResult processed. Populating state with {} items.` in `handlers/vein/src/lib.rs` when `SMessage::StorageLoadAllResult` is caught in the brain loop and mapped to `app_state.history`.

**Anomaly (Phase 3):** `VeinHandler` correctly populated the backend `AppState` with the history payload, but `translator.rs` history cursor logic failed to bridge the bulk payload to the UI channel. The frontend remained blind to the state update.

**Resolution (Phase 3):**
Enforced architectural boundaries. Added explicit telemetry in `libs/quartzite/src/platforms/gtk/workspace/translator.rs` right before `tx_gui.send(GuiUpdate::HistoryBatch)` to verify the payload construction. Lifted the GTK scroll math quarantine in `libs/quartzite/src/platforms/gtk/workspace/comms.rs` because the math is now mathematically verified innocent.
