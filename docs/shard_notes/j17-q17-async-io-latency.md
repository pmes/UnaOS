<!--
    This file is part of UnaOS.
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

## 2026-03-19 - Draft Autosave Latency Offloaded

**Anomaly:** Synchronous `std::fs::write` and `std::fs::remove_file` calls on the GTK main thread were causing frame drops and input stutter during composer autosaves and deletions.

**Resolution:** Offloaded all disk I/O to `tokio::task::spawn_blocking` to protect the UI thread's render cycle. The `PathBuf` and `String` variables were explicitly cloned right before being moved into the spawned background thread closures, satisfying the `Send + 'static` trait bounds while adhering to the zero-copy/move principle across the thread boundary. Any ephemeral filesystem lock errors are intentionally and safely ignored as drafts are transient.
