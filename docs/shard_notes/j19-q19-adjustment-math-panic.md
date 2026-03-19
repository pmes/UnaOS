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

## 2026-03-19 - [J19 "Euclid" :: Eradicating gtk_adjustment_configure Math Panics]

**Anomaly:** The GTK layout engine was repeatedly emitting `gtk_adjustment_configure` math panics during initial startup and window resize events. The existing adjustment signal listeners in the terminal (`libs/quartzite/src/platforms/gtk/workspace/comms.rs`) were naively calling `adj.set_value()` without performing strict geometric boundary checks against the incoming `upper`, `page_size`, and `lower` geometries.

**Resolution:** Instead of injecting fake geometries to trick the layout engine, we implemented strict mathematical constraints to respect the GTK boundaries. The `notify::upper` and `notify::page-size` event handlers now calculate the actual physics of the scroll interaction:
1. `is_at_bottom` is calculated using a 1-pixel floating point tolerance against the *old* layout states before the update.
2. If `new_upper >= lower + new_page_size` (content overflows viewport):
   - We strictly clamp to `new_upper - new_page_size` only if the user was already `is_at_bottom`.
   - If not `is_at_bottom`, we do nothing, actively respecting the user's manual scroll position.
3. If `new_upper < lower + new_page_size` (content fits entirely within the viewport):
   - We strictly clamp the adjustment value to `lower`, ensuring the view rests cleanly at the top bound without overflowing.
All calculations are safely deferred to the GTK idle loop to guarantee layout resolution before scroll actuation. The data-layer `boot_obj` was retained to ensure the initial list configuration yields a non-zero layout bound from the engine.