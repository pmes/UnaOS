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
## 2026-03-14 - The Legacy Reaper

**Anomaly:** The `GuiUpdate` enum was bleeding out of the hardware abstraction layer (`gneiss_pal`) and polluting the entire operating system logic (Vein, Qt, macOS). It forced cross-thread GUI compilation patterns on declarative frameworks. Furthermore, the massive `tokio::spawn` loops inside `VeinHandler` are mathematically complex and cannot be structurally refactored via simple string replacements without violating explicit shadow boundaries.

**Resolution:** We created `bandy::state::AppState` as the central source of truth and wrapped it in a fast synchronous `Arc<RwLock<AppState>>`. We completely ripped `GuiUpdate` out of the logic stack and relocated it physically into the legacy `libs/quartzite/src/platforms/gtk/` basement. The Synapse `SMessage::StateInvalidated` ping now triggers synchronous data-binding on the client UI loops. Finally, we rebuilt `VeinHandler::new` by hand to adhere to the Can-Am structural execution paths (The Shadow Boundary, The Lexical Lock, The Ping Release), fully appeasing the Rust compiler. `patch_gnome.py` hack script deleted.
