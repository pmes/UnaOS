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
use unaos_boot_info::{FrameBufferInfo, PixelFormat};

lazy_static! {
    pub static ref WRITER: Mutex<Writer> = Mutex::new(Writer::new());
}

pub struct Writer {
    framebuffer: Option<&'static mut [u8]>,
    info: Option<FrameBufferInfo>,
}

// Ensure Writer is Send
unsafe impl Send for Writer {}

impl Writer {
    pub fn new() -> Self {
        Self {
            framebuffer: None,
            info: None,
        }
    }

    pub fn init(&mut self, framebuffer: &'static mut [u8], info: FrameBufferInfo) {
        self.framebuffer = Some(framebuffer);
        self.info = Some(info);
    }

    pub fn width(&self) -> usize {
        self.info.as_ref().map(|i| i.width as usize).unwrap_or(0)
    }

    pub fn height(&self) -> usize {
        self.info.as_ref().map(|i| i.height as usize).unwrap_or(0)
    }

    pub fn write_pixel(&mut self, x: usize, y: usize, color: u32) {
        if let (Some(fb), Some(info)) = (&mut self.framebuffer, &self.info) {
            let offset = (y * info.stride + x) * info.bytes_per_pixel;
            if offset + info.bytes_per_pixel <= fb.len() {
                let r = ((color >> 16) & 0xFF) as u8;
                let g = ((color >> 8) & 0xFF) as u8;
                let b = (color & 0xFF) as u8;
                
                match info.pixel_format {
                    PixelFormat::Rgb => {
                        fb[offset] = r;
                        fb[offset + 1] = g;
                        fb[offset + 2] = b;
                    }
                    PixelFormat::Bgr => {
                        fb[offset] = b;
                        fb[offset + 1] = g;
                        fb[offset + 2] = r;
                    }
                    PixelFormat::U8 => {
                        fb[offset] = ((r as u16 + g as u16 + b as u16) / 3) as u8;
                    }
                    _ => {}
                }
            }
        }
    }
}
