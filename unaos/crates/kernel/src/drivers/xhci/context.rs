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


/// Context stride in u32 words. HCCPARAMS1.CSZ selects the controller's context size: CSZ=0 →
/// 32-byte contexts (QEMU, Intel Panther Point, VL805), CSZ=1 → 64-byte (Tegra234 XUSB — its FW
/// rejects 32-byte-stride input contexts with completion code 17 Parameter Error at
/// ADDRESS_DEVICE; JB9 bench 2026-07-08). The meaningful dwords sit in the first 8 words either
/// way, so field writes are stride-agnostic — only the array shape the hardware walks changes.
/// Compile-time by feature: the tegra kernel always drives the CSZ=1 XUSB block; every other
/// build (x86, pi4, QEMU regressions) stays byte-identical at 32.
#[cfg(feature = "tegra")]
pub const CTX_WORDS: usize = 16;
#[cfg(not(feature = "tegra"))]
pub const CTX_WORDS: usize = 8;

#[derive(Debug, Clone, Copy)]
#[repr(C, align(64))]
pub struct DeviceContext {
    pub slot: [u32; CTX_WORDS],      // Slot Context
    pub ep0:  [u32; CTX_WORDS],      // Endpoint 0 Context
    pub eps:  [[u32; CTX_WORDS]; 30] // Endpoints 1-30
}

impl DeviceContext {
    pub const fn new() -> Self {
        Self {
            slot: [0; CTX_WORDS],
            ep0: [0; CTX_WORDS],
            eps: [[0; CTX_WORDS]; 30],
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(C, align(64))]
pub struct InputContext {
    pub control: [u32; CTX_WORDS],   // Input Control Context
    pub device:  DeviceContext
}

impl InputContext {
    pub const fn new() -> Self {
        Self {
            control: [0; CTX_WORDS],
            device: DeviceContext::new(),
        }
    }
}
