// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.


#[derive(Debug, Clone, Copy)]
#[repr(C, align(64))]
pub struct DeviceContext {
    pub slot: [u32; 8],      // Slot Context (32 bytes)
    pub ep0:  [u32; 8],      // Endpoint 0 Context (32 bytes)
    pub eps:  [[u32; 8]; 30] // Endpoints 1-30 (30 * 32 bytes)
}

impl DeviceContext {
    pub const fn new() -> Self {
        Self {
            slot: [0; 8],
            ep0: [0; 8],
            eps: [[0; 8]; 30],
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(C, align(64))]
pub struct InputContext {
    pub control: [u32; 8],   // Input Control Context (32 bytes)
    pub device:  DeviceContext
}

impl InputContext {
    pub const fn new() -> Self {
        Self {
            control: [0; 8],
            device: DeviceContext::new(),
        }
    }
}
