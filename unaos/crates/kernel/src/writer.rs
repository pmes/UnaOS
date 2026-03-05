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

use lazy_static::lazy_static;
use spin::Mutex;

// STUB: We are disabling FrameBuffer graphics for the Bootloader 0.9 downgrade.
// We will rely on Serial output for the first successful boot.

lazy_static! {
    pub static ref WRITER: Mutex<Writer> = Mutex::new(Writer {});
}

pub struct Writer;

impl Writer {
    // Stub init function that does nothing
    // We accept arguments that match main.rs but ignore them
    pub fn init(&mut self, _buffer: &'static mut [u8], _info: impl AnyIgnore) {
        // No-op
    }

    pub fn width(&self) -> usize {
        0
    }
    pub fn height(&self) -> usize {
        0
    }
    pub fn write_pixel(&mut self, _x: usize, _y: usize, _color: u32) {}
}

// Helper to swallow the type mismatch in main.rs without editing main.rs again
pub trait AnyIgnore {}
impl<T> AnyIgnore for T {}
