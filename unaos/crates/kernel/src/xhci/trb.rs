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
#[repr(C, packed)]
pub struct Trb {
    pub parameter: u64,
    pub status: u32,
    pub control: u32,
}

impl Trb {
    pub const fn new() -> Self {
        Self { parameter: 0, status: 0, control: 0 }
    }

    // A "No Op" command is the safest way to test the ring.
    // Type ID for No Op is 23.
    pub fn new_noop(cycle_bit: bool) -> Self {
        let mut t = Self::new();
        // TRB Type 23 starts at bit 10 of the control field
        // Bit 5 is IOC (Interrupt On Completion)
        // Cycle bit is bit 0
        let type_val = 23u32 << 10;
        let ioc = 1u32 << 5;
        let cycle = if cycle_bit { 1 } else { 0 };

        t.control = type_val | ioc | cycle;
        t
    }
}
