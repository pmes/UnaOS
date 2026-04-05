// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

// libs/gneiss_pal/src/lib.rs (Logic Kernel)
#![allow(deprecated)]

pub mod api;
pub mod forge;
pub mod io;
pub mod paths;
pub mod persistence;
pub mod utils;
pub mod app_handler;

// Re-export types so consumers see them at the root
pub use utils::*;
pub use app_handler::*;

// --- LOGIC KERNEL ---
// No GTK, No Assets, No UI.
