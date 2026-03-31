<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright (C) 2026 The Architect & Una

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.
-->

# 🧠 J4 "The Scalpel" - Operational Report & Shard Notes
**Branch:** `j4-q4-lumen-stabilization`

## Operational Summary
This operation evolved from a simple UI bug squashing session into a deep-dive architectural recovery spanning the Tokio asynchronous reactor, Qt 6 threading semantics, CXX-Qt bridging memory ownership, QML rendering loops, and the UnaFS structural cataloging index.

## Key Anomalies & Resolutions

### 1. The Cross-Thread SIGINT Deadlock
**Problem:** To prevent dirty mounts, we mapped the Tokio OS signal interceptor to gracefully drop background filesystem loops. However, we attempted to terminate the Qt event loop directly from the background Tokio thread via `QCoreApplication::quit()`, resulting in a devastating thread deadlock that permanently hung the terminal.
**Resolution:** Qt strictly prohibits mutating the application state from non-GUI threads. We utilized `QMetaObject::invokeMethod(qApp, "quit", Qt::QueuedConnection)` to safely post an event to the main Qt loop, breaking the deadlock.

### 2. The UnaFS Dirty Mount (Torn Transactions)
**Problem:** Even after fixing the Qt loop, the app continued throwing `DIRTY MOUNT DETECTED` because `main.rs` was calling `std::process::exit(0)` to reap the background processes. This OS kill bypassed Rust's `Drop` traits, destroying the UnaFS Write-Ahead Log mid-flush.
**Resolution:** We utilized Tokio's native `.abort()` to gracefully cancel the infinite core logic loop. We then held the main thread alive with a 1000ms `std::thread::sleep` immediately after the abort. This provided the exact window necessary for the unspooled Drop traits to synchronously flush block devices back to the OS before teardown completed.

### 3. Null Disconnects & C++ Teardown Sequence
**Problem:** `QObject::disconnect: Unexpected nullptr parameter` was triggering because the CXX-Qt models owned by Rust were dropping *after* the QML engine attempted to evaluate them on exit.
**Resolution:** We manually intervened in the `LumenMainWindow::~LumenMainWindow()` destructor. We explicitly called `m_quickWidget->disconnect()` and `delete m_quickWidget` to wipe the QML environment before the C++ class dropped its injected data models.

### 4. QML Layout Anchor Loops & Theming
**Problem:** The Pre-Flight overlay was unreadable in Light Mode, and its text fields disappeared due to a negative-width collapse.
**Resolution:** We discovered that explicitly applying `anchors.fill: parent` to a `TextArea` nested inside a `ScrollView` triggers a catastrophic layout loop. Once removed, the fields sized properly. We also stripped all hardcoded hex colors (`#1e1e1e`, `#FFFFFF`), relying instead on `SystemPalette { id: sys; colorGroup: SystemPalette.Active }`. This allowed the QML components to natively map to the OS-level GTK/Wayland light and dark themes via `sys.window`, `sys.text`, and `sys.base`.

### 5. Popup Conversions vs Property Aliases
**Problem:** The `UnaDialog` component crashed the app ("White Screen of Death") because converting it from a basic `Item` to a native `Popup` accidentally wiped out its required custom `buttons` property.
**Resolution:** We mapped the `Popup` content to a `ColumnLayout` and explicitly redefined the properties `property var buttons: []` inside the root object. We also learned that clearing QML `TextArea` components by their external alias boundaries within a dynamic signal often fails; we must target the local property ID directly (`systemTextArea.text = ""`) to safely scrub the visual state.

### 6. The Ghost History (The UnaFS Indexing Bug)
**Problem:** The app successfully saved memories (verifiable via the PreFlight search context), but booted with `VAULT IS EMPTY`, failing to display the chat history in the UI. We initially suspected broken CXX-Qt model parameters.
**Resolution:** The issue was rooted deep within the database layer. `UnaFS` `create_inode` writes raw attributes but does not update the searchable catalog index. Because `save_memory` never explicitly called `set_attribute` for the `type == "chat"` variable, the boot loader's `query("type == \"chat\"")` request mathematically returned 0 results. Explicitly assigning the type via `set_attribute` forced the database to index the record, instantly rendering the missing bubbles.

---

## 🚀 Strategic Directives for J5 (The Deep Fix)

To the incoming Shard (J5), proceed with the following mandatory architectural focal points based on J4's final discoveries. The application is currently suffering from a broken teardown sequence, fractured UI theming, and an isolated popup abort bug that J4 failed to resolve before turnover.

### 1. The Persistent Dirty Mount (Tokio Drop Failure)
**Symptom:** `[WARNING] :: DIRTY MOUNT DETECTED. TORN TRANSACTION IN JOURNAL.`
**J4's Discovery:** The attempt to use `core_handle.abort()` followed by a `std::thread::sleep` in `main.rs` failed to trigger the `UnaFS` flush. Tokio's `.abort()` mechanism immediately stops task execution but does NOT guarantee that the internal `DiskManager` inside `VeinHandler` drops cleanly if the outer Tokio runtime is simultaneously torn down or if the `MutexGuard` across await points prevents unwinding.
**J5 Action Required:** Do not rely on `.abort()`. You must implement a clean shutdown signal into the `core::ignite` loop itself so it can `break` naturally, allowing variables to fall out of scope and explicitly execute their `Drop` traits before the runtime exits.

### 2. QML Theming (The Primitive Collapse)
**Symptom:** "Not working with light/dark mode." The screen rendered black text on dark backgrounds ("NOTHING?").
**J4's Discovery:** J4 attempted to strip all hardcoded hex colors and apply `SystemPalette`. However, primitive QML elements like `Text` and `Rectangle` do not automatically inherit Qt Quick Controls 2 styles. They defaulted to `#000000` (black) text, which vanished against the dark system window background. Furthermore, `SystemPalette` behavior can be inconsistent across Wayland/X11 platforms if the Qt platform theme isn't explicitly configured.
**J5 Action Required:**
- Convert raw `Text` primitives to `Label` components (which natively track the system text color).
- Convert `Rectangle` backgrounds to `Pane`, `Frame`, or `ItemDelegate` where native themed backgrounds are required.
- Do not blindly `sed` delete `color:` properties without replacing the underlying primitive type.

### 3. The Broken Cancel Popup
**Symptom:** "Cancel not working." The Cancel button opens the popup, but the "reject" action fails to clear the UI or backend.
**J4's Discovery:** Converting `UnaDialog` to a native `Popup` solved centering but detached the QML property scope from the parent `PreFlightOverlay`. The `onActionTriggered` signal is firing, but referencing `systemTextArea.text = ""` or `root.backend` from *inside* the generic `UnaDialog` instance scope fails because it cannot natively reach the parent overlay's explicit IDs without proper alias routing or explicit signal handling on the parent level.
**J5 Action Required:** Do not clear the `TextArea` components from inside the `UnaDialog` definition. Instead, bind the `onActionTriggered` signal at the instantiation site inside `PreFlightOverlay.qml`, intercept the `"reject"` string, and execute the clear/abort logic there where the component IDs are safely in scope.