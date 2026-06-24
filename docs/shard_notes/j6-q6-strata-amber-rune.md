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

# 🧠 J6 "Strata" :: UNAOS SHARD DIRECTIVE
**Designation:** J6 "Strata" 🪨
**Role:** The Storage Rune Forger

## Architectural Learnings: Transitioning to the Asynchronous Rune Model

### The Monolith's Weakness
Prior to this transition, the `Vein` matrix possessed explicit, physical ownership over the `DiskManager` inside `handlers/vein/src/storage.rs`. This tight coupling resulted in synchronous wait locks over the heavy IO-bound `UnaFS` system while Vein was concurrently responsible for servicing real-time UI interactions through `bandy`.

### The Can-Am Rune Architecture
We have successfully extracted the `DiskManager` and established a physically separate Rune inside `handlers/amber_bytes`. A Rune represents absolute isolation. Amber Bytes no longer communicates through explicit function calls from Vein; instead, the logic flows through the `Synapse` nervous system.

#### The Receipt System
By introducing `receipt_id` into the `SMessage` variants:
- **`StorageQuery`**
- **`StorageQueryResult`**
- **`StorageSave`**
- **`StorageSaveResult`**
- **`StorageLoadAll`**
- **`StorageLoadAllResult`**

We have solved the primary asynchronous deadlock issue. Vein can immediately resume other duties (like rendering the UI) rather than waiting blockingly. When Amber Bytes responds back into the `Synapse`, Vein acts as a non-blocking receiver in its async `tokio::select!` loops.

#### Shared Memory Schemas (`libs/bandy`)
As Amber Bytes and Vein both manipulate the same `DispatchRecord`, this struct and the `SMessage` variants were moved directly into the universal language definition library `bandy`. This guarantees zero-copy, pure zero-cost transitions across the boundaries of the components.

#### Absolute Disassembly
The `/clear` command in Vein has been eliminated. Modifying the base mount point natively inside `vein` violated the Actor model's isolation constraints. The Amber Rune owns its disk block fully and completely.