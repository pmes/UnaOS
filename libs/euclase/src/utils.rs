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

use core::f32::consts::PI;

/// Converts degrees to radians.
#[inline]
pub fn to_radians(degrees: f32) -> f32 {
    degrees * (PI / 180.0)
}

/// Converts radians to degrees.
#[inline]
pub fn to_degrees(radians: f32) -> f32 {
    radians * (180.0 / PI)
}
