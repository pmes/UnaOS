<!--
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

## 2026-10-27 - GTK4 Layout Matrix Crash (Empty ListView Scroll Adjustments)

**Anomaly:** `gtk_adjustment_configure: assertion 'lower + page_size <= upper' failed`
When a dynamically mapped GTK4 `ListView` (driven by a `SignalListItemFactory` and `NoSelection` over a `gio::ListStore`) initializes entirely empty inside a `ScrolledWindow`, its bounds evaluate to a perfect 0x0 void. If the parent `ScrolledWindow` is set to expand (`vexpand`), or if a wrapper container attempts to shrink-wrap the empty list (`valign(Align::End)`), GTK tries to execute the adjustment math `0 + viewport_height <= 0`. This mathematical paradox throws a `Gtk-CRITICAL` C-level assertion error, which silently aborts the render pass and leaves the window completely blank or frozen, preventing even initial system boot logs from appearing.

**Resolution (Current State & Challenges):**
I attempted a multi-layered Can-Am bypass strategy to un-brick the layout engine during boot:
1. **Explicit Geometries:** I stripped out contradictory explicit sizing (like `min_content_height(400)` combined with empty lists) and removed toxic alignments like `Align::End` that forced the viewport to crush.
2. **Layout Shields:** I wrapped the `ListView` in an intermediate `gtk::Box` container and appended a 1x1 transparent `spacer`, tricking the `ScrolledWindow` into evaluating a 1-pixel height `upper` boundary.
3. **Signal Delays:** I moved the `connect_value_notify` and `connect_upper_notify` `vadjustment` triggers inside `.connect_map()` to delay the mathematical calculations until the window was physically drawn on screen.
4. **Data Seed:** The current codebase attempts to bypass this by unconditionally appending a "Boot" telemetry string to the `console_store` before `connect_map` runs, theoretically ensuring the `ListView` is never 0px high on its first layout pass.

**Suggestions for J11 (The Final Lock):**
The UI still refuses to draw historical chat bubbles despite the system log functioning. The problem is a desync between the frontend list store memory allocations and the backend `AppState`.

J11, when you pick this up, look critically at `translator.rs` and the `reactor.rs` loop.
The current `reactor.rs` logic implements two distinct `GuiUpdate` handlers for lists: `ConsoleLogBatch` and `HistoryBatch`.
* You need to examine how the `AppState.history` is being translated across the channel. Are historical bubbles dropping because they arrive during the blocked `ScrolledWindow` shadow-mapping pass?
* Do not revert to `FilterListModel` or DOM-traversal (`.next_sibling()`). Keep the `glib::BoxedAnyObject` pointer packing in `comms.rs`.
* Consider evaluating if the `glib::idle_add_local` scroll adjustment delay inside `comms.rs` is inadvertently masking or blocking the render of the `HistoryBatch` splice by consuming the UI thread cycle. If the batch splice occurs but the idle loop fails or blocks layout reallocation, GTK may discard the visual nodes.