#![no_std]
#![no_main]

/*
    unaOS Core System
    Copyright (C) 2026  The unaOS Contributors

    This program is free software: you can redistribute it and/or modify
    it under the terms of the GNU General Public License as published by
    the Free Software Foundation, either version 3 of the License, or
    (at your option) any later version.

    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
    GNU General Public License for more details.
*/

extern crate alloc;

use alloc::vec;
use uefi::prelude::*;
use uefi::println;
use uefi::proto::console::gop::{GraphicsOutput, PixelFormat};
use uefi::table::boot::MemoryType;
use uefi_services as _;

mod font;
mod graphics;

use graphics::FramebufferWriter;

#[entry]
fn main(_image_handle: Handle, mut system_table: SystemTable<Boot>) -> Status {
    uefi::helpers::init(&mut system_table).unwrap();

    println!("[ unaOS ] System Online. Hardware Handoff Complete.");

    let boot_services = system_table.boot_services();

    let mmap_size = boot_services.memory_map_size();
    let mut mmap_buffer = vec![0; mmap_size.map_size + 8 * mmap_size.entry_size];

    let memory_map = boot_services
        .memory_map(&mut mmap_buffer)
        .expect("Failed to retrieve memory map");

    for descriptor in memory_map.entries() {
        if descriptor.ty == MemoryType::CONVENTIONAL {
            let start = descriptor.phys_start;
            let size_bytes = descriptor.page_count * 4096;
            let size_kb = size_bytes / 1024;

            println!("[ RAM ] 0x{:016x} - {} KiB", start, size_kb);
        }
    }

    // Task: GOP Initialization
    let gop_handle = boot_services
        .get_handle_for_protocol::<GraphicsOutput>()
        .expect("Failed to locate GraphicsOutput protocol handle");

    let mut gop = boot_services
        .open_protocol_exclusive::<GraphicsOutput>(gop_handle)
        .expect("Failed to open GraphicsOutput protocol");

    let mode_info = gop.current_mode_info();
    let (width, height) = mode_info.resolution();
    let stride = mode_info.stride();
    let pixel_format = mode_info.pixel_format();

    let mut fb = gop.frame_buffer();
    let fb_ptr = fb.as_mut_ptr();
    let fb_base = fb_ptr as u64;

    println!(
        "[ GOP ] Resolution: {}x{}, Stride: {}, Base: 0x{:016x}",
        width, height, stride, fb_base
    );

    // Task: The Proof (Blue Screen of Life)
    // We assume 4 bytes per pixel for RGB/BGR modes.
    let bytes_per_pixel = 4;

    for y in 0..height {
        let row_offset = y * stride;
        for x in 0..width {
            let pixel_offset = (row_offset + x) * bytes_per_pixel;

            unsafe {
                let pixel_ptr = fb_ptr.add(pixel_offset);
                match pixel_format {
                    PixelFormat::Rgb => {
                        // Blue: [0, 0, 255]
                        pixel_ptr.write_volatile(0);
                        pixel_ptr.add(1).write_volatile(0);
                        pixel_ptr.add(2).write_volatile(255);
                    }
                    PixelFormat::Bgr => {
                        // Blue: [255, 0, 0]
                        pixel_ptr.write_volatile(255);
                        pixel_ptr.add(1).write_volatile(0);
                        pixel_ptr.add(2).write_volatile(0);
                    }
                    _ => {
                        // Grey: [128, 128, 128] for unsupported/other
                        pixel_ptr.write_volatile(128);
                        pixel_ptr.add(1).write_volatile(128);
                        pixel_ptr.add(2).write_volatile(128);
                    }
                }
            }
        }
    }

    // Task: The Voice (Font Rendering)
    let mut writer = FramebufferWriter::new(fb_ptr, stride, pixel_format, bytes_per_pixel);

    // Draw "unaOS v0.0.2" in White at (50, 50)
    writer.draw_string(50, 50, "unaOS v0.0.2", [255, 255, 255]);

    loop {}
}
