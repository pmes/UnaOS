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

# 🧠 J8 "Boddyknock" :: UNAOS SHARD DIRECTIVE

## 2026-10-24 - The Structural Harmonizer

**Anomaly:** The GTK/GNOME monolith (`libs/quartzite/src/platforms/gtk/spline.rs`) collapsed into a 1,300-line gravity well, entangling sidebar widgets, communication loops, massive inline CSS blocks, and direct `bandy::SMessage` parsers into single, unmaintainable closures.

**Resolution:** Executed a surgical Can-Am extraction:
1.  **CSS Bloat Removed:** Extracted all inline `.una-dark`, `.builder-sidebar`, and fallback `headerbar` overrides out of `mega_bar.rs` and `spline.rs` and natively injected them into `libs/quartzite/src/platforms/gtk/assets/style.css` via `/org/una/vein` GResource targeting.
2.  **Lobe Severance:** Sliced `spline.rs` down to ~80 lines by generating a pristine `workspace/` hierarchy. `sidebar.rs` isolates the nodes, nexus spinner arrays, and teleHUD vectoring. `comms.rs` orchestrates the chat, composer, and input popovers.
3.  **The Translator & Reactor (The Synapse Routing Fix):** Created a pure `translator.rs` node that strictly converts `SMessage` packets into UI-safe `GuiUpdate` enums. It then funnels this to a unified `reactor.rs` loop holding cleanly grouped structs of `Rc<RefCell<...>>` UI pointers, permanently preventing silent multiple-consumer channel blocking on `rx_synapse`.
4.  **Signature Harmony:** Symmetrical wrappers inside `spline.rs` now just bootstrap the `workspace::build()` return widgets directly into their platform-specific GNOME / GTK outer `MegaBar` boundaries.

### Note to J12 (The Stubborn Render Bug)

**Anomaly:** While the structural dissection was pristine, there is an ongoing bug where UI ListBox renders drop content text, exclusively outputting generic `System - [Time]` blocks without their associated payloads or proper parsing.

**Diagnostic Warning for J12:**
The issue resides in `comms.rs` or `reactor.rs` during the mapping of the `DispatchObject` into the `console_store`.
- **Hypothesis 1:** When the `SMessage` is caught by `translator.rs`, the payload contents (e.g. `GuiUpdate::ConsoleLog(text)`) are not properly bubbling down through to the `item.set_child` logic in `comms.rs`.
- **Hypothesis 2:** The `SignalListItemFactory` in `comms.rs` binds `obj.content()`, but during the extraction process, it's possible `chat_view.buffer().set_text(content.trim_end());` is binding too late, or the `SourceView` parameters (height request, wrap modes) are collapsing visually because the paned window lost its inherited GTK geometry hints from `spline.rs`.

**Directive to J12:** Verify the `console_factory.connect_bind` closure inside `libs/quartzite/src/platforms/gtk/workspace/comms.rs`. Ensure `SourceView` visibility states aren't hiding the payload, and trace `GuiUpdate::ConsoleLog` and `GuiUpdate::HistoryBatch` variants entering `reactor.rs`.