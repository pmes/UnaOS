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

## 2023-10-27 - Crystallizing the Macro-Frame Layout (WorkspaceTetra)
**Anomaly:** Hardcoded pixel widths (e.g. 260px) were used to define layout splits in the GTK and Qt Embassies, violating resolution independence and tangling core logical layout with UI implementation.

**Resolution:** Abstracted the split frame layout into a pure Rust declarative UI API engine (`tetra.rs`). `WorkspaceTetra` now dictates `left_pane` and `right_pane` via pure logical enums (`TetraNode`), and defines the boundary using a proportional `split_ratio: f32` (e.g., 0.25). Native embassies interpret this ratio against absolute runtime window bounds (e.g., via `window.default_width()` in GTK, and `_splitRatio` context injection in Qt/QML).
