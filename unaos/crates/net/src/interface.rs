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

/// Represents a hardware-agnostic network device that can transmit and receive raw frames.
pub trait NetworkDevice {
    /// Transmits a raw network frame over the device.
    fn transmit(&mut self, buffer: &[u8]);

    /// Receives a raw network frame from the device, if available.
    fn receive(&mut self) -> Option<&[u8]>;

    /// Returns the physical MAC address of the device.
    fn mac_address(&self) -> [u8; 6];
}
