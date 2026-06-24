<!--
  SPDX-License-Identifier: LGPL-3.0-or-later
  Copyright (C) 2026 The Architect & Una

  This program is free software: you can redistribute it and/or modify
  it under the terms of the GNU Lesser General Public License as published by
  the Free Software Foundation, either version 3 of the License, or
  (at your option) any later version.

  This program is distributed in the hope that it will be useful,
  but WITHOUT ANY WARRANTY; without even the implied warranty of
  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
  GNU Lesser General Public License for more details.

  You should have received a copy of the GNU Lesser General Public License
  along with this program.  If not, see <https://www.gnu.org/licenses/>.
-->

## 2026-03-24 - Pathfinder Zero-Latency Root Resolution Pipeline

**Anomaly:** `vein` relied on checking if `path.exists()` for raw relative input strings. This is a brittle check that assumes the current working directory matches the user's intent, leading to spatial degradation and false negatives if the IDE was launched from a different folder than the project root. Also, storing full absolute paths in the `Matrix` DAG consumed unnecessary memory.

**Resolution:**
1. Implemented a zero-latency `find_workspace_root` pipeline in `libs/elessar` that traverses upwards to locate anchors (`MEMORIA.md`, `Cargo.toml`, etc.), defaulting to `current_dir()` as a fallback.
2. Cached the absolute path immutably inside `AppState.absolute_workspace_root` during the `lumen` boot sequence, wrapping it in an `Arc` for zero-copy thread-safe access.
3. Updated `vein` to auto-join relative inputs with the absolute workspace anchor, mathematically proving existence before emitting the *original relative input string* via `MatrixEvent::FocusSector`.
4. Refactored `matrix` `MatrixScanner` to aggressively strip the absolute anchor prefix from scanned nodes, ensuring the DAG topology only stores memory-efficient relative paths.
