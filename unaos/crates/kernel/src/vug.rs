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

// FIX: Use crate:: instead of unaos_kernel::
use bootloader_api::info::{FrameBufferInfo, PixelFormat};

use crate::pal::TargetPal;
use gneiss_pal::GneissPal;

const METER_SEGMENTS: usize = 10;
const SEG_HEIGHT: u32 = 10;
const SEG_SPACING: u32 = 2;
const SEG_WIDTH: u32 = 40;

pub fn init(framebuffer: &mut [u8], info: FrameBufferInfo) {
    let width = info.width;
    let height = info.height;
    let stride = info.stride;
    let format = info.pixel_format;

    crate::serial_println!(":: VUG Init ::");
    crate::serial_println!(":: FB Size: {}x{} (stride {}) ::", width, height, stride);
    crate::serial_println!(":: FB Format: {:?} ::", format);

    // Can-Am dark grey: #1E1E1E
    let r_val = 0x1E;
    let g_val = 0x1E;
    let b_val = 0x1E;

    let bytes_per_pixel = info.bytes_per_pixel;

    for y in 0..height {
        for x in 0..width {
            let offset = (y * stride + x) * bytes_per_pixel;
            if offset + bytes_per_pixel <= framebuffer.len() {
                match format {
                    PixelFormat::Rgb => {
                        framebuffer[offset] = r_val;
                        framebuffer[offset + 1] = g_val;
                        framebuffer[offset + 2] = b_val;
                        // Alpha/Reserved byte is unmodified or set to 0
                    }
                    PixelFormat::Bgr => {
                        framebuffer[offset] = b_val;
                        framebuffer[offset + 1] = g_val;
                        framebuffer[offset + 2] = r_val;
                    }
                    PixelFormat::U8 => {
                        // Greyscale approximation
                        framebuffer[offset] = 0x1E;
                    }
                    _ => {
                        // Unknown format, do not write
                    }
                }
            }
        }
    }

    crate::serial_println!(":: Framebuffer painted #1E1E1E ::");
}

pub fn draw_vug_stats(pal: &mut TargetPal, tick: u64) {
    // FIX: Removed extra parentheses
    let total_height = METER_SEGMENTS as u32 * (SEG_HEIGHT + SEG_SPACING);
    let start_x = pal.width() - SEG_WIDTH - 20;
    let start_y = pal.height() - total_height - 20;

    // Draw a specialized "VU Meter"
    for i in 0..METER_SEGMENTS {
        let active = (tick / 10) % (METER_SEGMENTS as u64);
        let color = if (i as u64) <= active {
            0x00FF00 // Green
        } else {
            0x333333 // Dim Gray
        };

        let y_pos = start_y + (i as u32 * (SEG_HEIGHT + SEG_SPACING));
        pal.draw_rect(
            start_x as usize,
            y_pos as usize,
            SEG_WIDTH as usize,
            SEG_HEIGHT as usize,
            color,
        );
    }

    // Draw "VUG" heartbeat
    if (tick / 30) % 2 == 0 {
        pal.draw_rect((pal.width() / 2) as usize - 10, 20, 20, 20, 0xFF0000);
    }
}
