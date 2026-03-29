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

## 2024-05-18 - [J3-Q3 Bismuth Fluid Glass Integration]

**Anomaly:** Routing raw payloads directly from the C++ engine to QML risks blocking the Tokio runtime during heavy transmission intervals, causing potential UI jitter. Moreover, modifying the core IPC `SMessage` to echo the payload back introduces unnecessary structural overhead and violates the architectural mandate to avoid touching the core IPC enums.

**Resolution:** Leveraged CXX-Qt 0.8 `#[qsignal]` to broadcast the raw payload directly from the `vein_bridge` `send_message` closure across the FFI boundary immediately *before* it is committed to the wire. Built a dedicated `NetworkLogModelRust` that consumes this signal in Qt space via QML `Connections`, caching the data strictly on the heap for zero-copy visual rendering. This provides a transparent truth view without polluting the autonomic routing system or mutating `SMessage`. Re-engineered `NexusChat.qml` logic to calculate layout fluid geometry mathematically using `Math.min()` bounded bounds, removing any need for legacy Qt layouts.

## 2024-05-18 - [J3-Q3 Pre-Flight Geometry and Primitive Restorations]

**Anomaly:** Initial UI configurations exhibited deep architectural logic flaws (e.g. binding loops preventing user edit capabilities inside the pre-flight overlay, Qt 5 MessageDialog legacy crashes preventing window rendering, and multiple instantiated bridges generating a CXX-Qt `OnceLock` fracture).

**Resolution:** Repaired `NexusChat.qml` by eradicating multiple `VeinBridge` invocations in favor of routing a global generic `property var backend`. Fixed pre-flight bindings by utilizing dynamic `property alias` parameters and initializing string overrides strictly on the event loop closure `onPayloadReadyForReview`. Successfully integrated `QtQuick.Controls` Dialog (modal overlay centering perfectly within the application layout to prevent Wayland pop-out behavior). Implemented Can-Am reset hack (`begin_reset_model()`) to safeguard model insertions. Enabled TextEdit UI hooks inside Truth View.

***

### 📝 RECOMMENDATIONS FOR J4 :: DEEP SCAN ANALYSIS

**1. Target Anomaly: `[WARNING] :: DIRTY MOUNT DETECTED. TORN TRANSACTION IN JOURNAL.`**
*   **Analysis:** The `unafs` crate logs this warning upon boot (`libs/unafs/src/fs.rs: mount()`) when the internal journal identifies an incomplete commit or torn transaction sequence across blocks during initialization.
*   **Investigation:** I confirmed that `UnaFS<D>` already explicitly implements the standard Rust `Drop` trait, correctly invoking `self.sync_metadata()` and `self.device.flush()` to ensure atomic saves during a graceful shutdown sequence. The occurrence of this warning in the console output indicates that the parent application (`lumen` / `vein` reactor) is encountering an ungraceful shutdown. This typically happens when the process panics, is externally killed via SIGKILL, or the Tokio runtime terminates before dropping its static/global variables correctly.
*   **J4 Strategy:**
    1.  **Intercept Signal Handlers:** Implement explicit OS signal handlers (SIGINT, SIGTERM) inside the primary application binary (`apps/lumen/src/main.rs`). Instead of terminating abruptly, capture the interrupt and initiate a graceful "Drain & Drop" sequence, halting new network traffic and awaiting the `UnaFS` storage `Drop` execution.
    2.  **Audit the Storage Lock:** If `UnaFS` is wrapped inside a global static `RwLock` or `Mutex` (e.g. `OnceLock`), Rust's global shutdown sequence *does not* automatically execute `Drop` traits on static variables. J4 must explicitly acquire the lock and `.take()` or manually drop the file system instance during the termination sequence.

**2. Target Anomaly: The Pre-Flight Inputs Not Populating/Accepting Input**
*   **Analysis:** J3 attempted multiple patterns to populate the fields. First, we mapped QML properties directly (`property var payloadModel` using `text: payloadModel ? payloadModel.system : ""`) which caused bidirectional `onTextChanged` binding loops that locked the QML evaluator and made the fields read-only. Second, we transitioned to the round-trip signal architecture (`requestPreFlightReview` -> core -> `payloadReadyForReview`). Third, we explicitly aliased the fields (`property alias systemTextAreaText: systemTextArea.text`). Despite these structural updates, The Architect reports the inputs are *still* not populating or accepting inputs.
*   **J4 Strategy:** J4 needs to trace the `Event::Input` ("chat") payload. In `vein_bridge.rs`, clicking "Pre-Flight" fires `request_pre_flight_review`, which sends `Event::Input` into the Tokio `GLOBAL_TX` channel. The core logic kernel (likely inside `libs/vein` or `libs/gneiss_pal`) is supposed to intercept this and return a `GuiUpdate::ReviewPayload`. Ensure the core is *actually* returning this `GuiUpdate`. If the reactor logic is swallowing the event without generating the payload, the `payloadReadyForReview` signal is never emitted to QML, resulting in blank, empty inputs. J4 must scan the logic kernel's `handle_event` for `Event::Input` and verify the `PreFlightPayload` struct is generated and routed back correctly.

**3. Target Anomaly: Pre-Flight Cancel Bad Design (The Popup)**
*   **Analysis:** The Architect explicitly noted that J3's implementation of the cancel dialog is a "bad design." J3 originally used `Qt.labs.platform` for a native OS compositor window. However, The Architect states: "cancel confirmation popup should popup over the top of the window, not the center of screen, and it should disable the app behind it. maybe there's already a special alert popup?"
*   **J4 Strategy:** J4 must immediately undo the `MessageDialog` implementation in `PreFlightOverlay.qml`. Do a full text scan across `libs/quartzite/src/platforms/qt/assets/qml/` and `libs/quartzite/src/widgets/` for an existing "special alert popup" component that The Architect built. If one exists, import and utilize it for the Cancel flow. Ensure it enforces `modal: true` to disable the parent app UI, and anchors to the top or center of the transient application window, rather than letting the host OS Window Manager float it loosely.

**4. Target Anomaly: Repeated "UNAFS VAULT EMPTY" Messages**
*   **Analysis:** `vein_bridge.rs::route_history_batch` pushes a synthetic ":: UNAFS VAULT EMPTY ::" visual confirmation into the UI list when the backend returns an empty history vector. The app is firing this multiple times on startup.
*   **J4 Strategy:** Ensure `route_history_batch` evaluates the current state of `HistoryModelRust.rows` before pushing the system message. Since the thread lock is asynchronous, wrap the injection inside the `thread.queue` closure so you can strictly check `if qobj.as_ref().rust().rows.is_empty() && rust_items.is_empty()` to prevent duplicate injections during multi-pulse initializations.

**5. Target Anomaly: Copying Text in the Network Log / Ghost Cursors**
*   **Analysis:** The `NetworkLogOverlay` currently utilizes a `Text` or primitive node to render the Truth View log strings, making them static and un-highlightable by the mouse.
*   **J4 Strategy:** Update the `delegate` inside `NetworkLogOverlay.qml`. Swap the component to `TextEdit`. Crucially, to adhere to the Can-Am visual constraints, explicitly set `readOnly: true`, `selectByMouse: true`, and `cursorVisible: false`. This allows native dragging/copying while eliminating the blinking GUI ghost cursor that clutters the visual truth stream.