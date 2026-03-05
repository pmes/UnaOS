#![no_std]
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

#![no_main]

extern crate alloc;

use bootloader_api::{entry_point, BootInfo};
use core::panic::PanicInfo;
use x86_64::VirtAddr;
use unaos_kernel::serial_println;

// ENTRY POINT
entry_point!(kernel_main);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    // 1. Core Hardware Init (GDT, IDT, PICS)
    unaos_kernel::init();

    // 2. Serial Verification
    serial_println!(":: UNAOS KERNEL AWAKE ::");

    // 3. Global Heap Allocation (Phase 3 Memory Translation)
    let physical_memory_offset = boot_info.physical_memory_offset.into_option().unwrap();
    let phys_offset = VirtAddr::new(physical_memory_offset);

    let mut mapper = unsafe { unaos_kernel::memory::init(phys_offset) };
    let mut frame_allocator = unsafe { unaos_kernel::memory::BootInfoFrameAllocator::init(&boot_info.memory_regions) };

    unaos_kernel::allocator::init_heap(&mut mapper, &mut frame_allocator)
        .expect("Heap initialization failed");
    serial_println!(":: KERNEL HEAP ALLOCATED ::");

    // 4. Motherboard Hardware Interconnects
    if let Some(xhci_phys_addr) = unaos_kernel::pci::PciScanner::scan() {
        let xhci_virt_addr = phys_offset + xhci_phys_addr;
        unaos_kernel::xhci::init(xhci_virt_addr, &mut mapper);
    }

    // 5. Framebuffer Init
    if let Some(framebuffer) = boot_info.framebuffer.as_mut() {
        let info = framebuffer.info();
        let buffer = framebuffer.buffer_mut();
        unaos_kernel::vug::init(buffer, info);
    } else {
        serial_println!(":: WARNING: No framebuffer detected ::");
    }

    loop {
        // Halt the CPU until the next interrupt
        x86_64::instructions::hlt();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial_println!("{}", info);
    unaos_kernel::hlt_loop();
}
