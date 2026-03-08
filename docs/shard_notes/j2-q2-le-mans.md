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

# J2 [VĀSCULĀRIS] Shard Report: Q2 Le Mans Operation

**Designation:** J2 "Vāsculāris"
**Status:** COMPLETE
**Reporting Line:** Una (Number One) & The Architect

## Mission Overview
The Q2 Le Mans Operation was a comprehensive architectural refactoring of the Lumen Qt/Rust hybrid presentation layer. The objective was to dismantle a rigid, monolithic FFI abstraction and establish a hyper-modular, extremely performant, and "Can-Am" compliant architecture bridging the asynchronous Tokio reactor (Rust) to the Qt Quick rendering layer (QML) using CXX-Qt 0.8.

## The Operational Hurdles & Solutions

### 1. The Monolithic Eradication
**Anomaly:** The legacy Qt implementation relied on a single `bridge.rs` file containing all structural abstractions, handler methods, and window logic, violating the Audi Le Mans Protocol for hyper-modularity.
**Resolution:** `bridge.rs` was fully dismantled and eradicated. It was replaced by a clean "Executive Router" (`window.rs`) responsible solely for Bandy IPC routing and global QML context initialization. Domain-specific structures and FFI models were cleanly segregated into Handler Facades (e.g., `vein_bridge.rs`). The global `spline.rs` was rewired to rely directly on the new Executive Router rather than preserving a hollowed-out Qt module.

### 2. The List Model Performance Mandate
**Anomaly:** Simple array or `QVariantList` serialization across the FFI border causes unacceptable memory bloat and UI stuttering in Qt when rendering long chat histories.
**Resolution:** We enforced the strict requirement to implement a true `QAbstractListModel` (HistoryModel) initialized entirely within Rust. The model overrides the Qt virtual functions (`rowCount`, `data`) explicitly using CXX-Qt's `cxx_override` macro mapping (`#[cxx_name = "rowCount"]`) to satisfy the strict Qt C++ compilation sequences without defaulting to expensive copy-evaluations.

### 3. The Abstract Class Compilation Trap
**Anomaly:** CXX-Qt requires strict syntactic adherence for subclassing. Defining `#[base = "QAbstractListModel"]` as a string literal caused a macro panic, and failing to explicitly define `<QtCore/QAbstractListModel>` in the `unsafe extern "C++"` block caused the GNU linker to panic due to an undefined opaque type.
**Resolution:** The inheritance macro was rewritten to use a raw Rust identifier (`#[base = QAbstractListModel]`). The correct native Qt Core header was aggressively included inside the bridge module space to guarantee the C++ compiler possessed the necessary type definitions prior to evaluating the generated headers.

### 4. QML Memory Lifecycle Safety (Dangling Pointers)
**Anomaly:** When initializing the QML environment, allocating the Rust-backed `HistoryModel` on the local C++ constructor stack (`HistoryModel historyModel;`) resulted in the objects instantly perishing after boot, leaving the `QQmlEngine` reading dangling pointers and resulting in silent UI black-screens.
**Resolution:** The models were securely elevated to the C++ heap (`new HistoryModel(this)`) to bind them permanently to the application's lifecycle, and explicitly mapped to the QML context properties (`QQmlContext::setContextProperty`).

### 5. The Qt Signal Router Crash (The Reset Hack)
**Anomaly:** When the Tokio reactor queried UnaFS and pumped historical memory into the model via `begin_insert_rows(parent)`, the default `QModelIndex` bridging triggered a devastating `Unexpected nullptr parameter` within Qt's internal signal dispatcher. Furthermore, Qt recursively queried the flat list as if it were a deeply nested fractal tree, searching for ghost children.
**Resolution:** Implemented the "Can-Am Reset Hack". `begin_insert_rows` was entirely removed from the FFI. Memory batch operations were heavily simplified to the argument-free `begin_reset_model()` and `end_reset_model()`. The `rowCount` implementation was strictly locked to return `0` if `parent.is_valid()`, completely halting the recursive fractal tree evaluation and unblocking the render thread.

### 6. The Native Role Bypass (QHash Expansion Failure)
**Anomaly:** The CXX-Qt 0.8 macro rejected standard Rust HashMap logic for resolving the `roleNames` FFI override.
**Resolution:** Instead of writing complex template expansions, we hijacked `QAbstractItemModel`'s natively exposed default C++ roles. We bound QML strictly to `model.display`, `model.decoration`, `model.edit`, and `model.toolTip`. A critical catch was ensuring that string text was passed exclusively to `DisplayRole` or `EditRole`, as passing it to `DecorationRole` forced QML to look for a non-existent `QIcon`/`QPixmap`, which silently evaluated to a 0x0 height frame and blacked out the UI.

### 7. Layout Singularities & Geometry Collapse
**Anomaly:** In QML, embedding a native Flickable (`ListView`) inside a `ScrollView` while issuing `anchors.fill: parent` generates a circular geometry evaluation. The UI completely collapsed to 0 pixels horizontally and vertically.
**Resolution:** Eradicated the redundant `ScrollView`. Made the `ListView` a direct sibling in the ColumnLayout, passing `Layout.fillHeight: true`. Implemented strict negative-space calculation safeguards (`Math.max(parent.width - 16, 10)`) within the delegate to mathematically prohibit the Scene Graph from panicking during early uninitialized bounds checking.

### 8. Static GNU Linker Registration
**Anomaly:** Trusting the auto-generated `.qmldir` plugin logic inside a statically linked Ubuntu GNU environment resulted in a complete module resolution failure and an aborted Qt engine.
**Resolution:** Restored manual registration calls (`qmlRegisterUncreatableType`) immediately following `cxx_qt_init_crate_quartzite()` in `main_window.cpp`. This forces the compiler to structurally bind the Rust models into the Meta-Object tree before loading `main.qml`, bypassing the faulty QRC plugin loader entirely.

## Conclusion
The data is on the glass. The Can-Am FFI architecture holds zero-copy references, routes dynamically without memory leaks, and operates cleanly alongside the legacy GTK environment. The Lumen QML layer is fundamentally decoupled from business logic and explicitly relies on rapid Tokio event cascades for real-time operation.
