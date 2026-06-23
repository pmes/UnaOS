#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

extern crate alloc;

use core::panic::PanicInfo;
use unaos_kernel::serial_println;
use unaos_boot_info::BootInfo;

#[unsafe(no_mangle)]
pub extern "C" fn _start(boot_info: &'static mut BootInfo) -> ! {
    // 1. Core Hardware Init (GDT, IDT, PICS for x86_64, GIC for aarch64)
    unaos_kernel::init();

    // 3. Framebuffer Info Extraction
    // Extract info before memory initialization consumes the BootInfo reference
    let framebuffer_addr = boot_info.framebuffer_addr;
    let framebuffer_size = boot_info.framebuffer_size;
    let info = boot_info.framebuffer_info;

    // 4. Global Heap Allocation (Phase 3 Memory Translation)
    // Architecture-specific initialization handles reading the memory map
    // and setting up the page tables and allocator.
    unaos_kernel::arch::memory::init(boot_info);
    serial_println!(":: KERNEL HEAP ALLOCATED ::");

    // 5. Motherboard Hardware Interconnects
    unaos_kernel::arch::pci::init();

    if framebuffer_addr != 0 {
        // Safety: We assume the bootloader passed a valid framebuffer physical address
        let buffer = unsafe {
            core::slice::from_raw_parts_mut(framebuffer_addr as *mut u8, framebuffer_size)
        };
        
        unaos_kernel::vug::init(&mut *buffer, info);
        
        unaos_kernel::writer::WRITER.lock().init(buffer, info);
    } else {
        serial_println!(":: WARNING: No framebuffer detected ::");
    }

    let mut console = unaos_kernel::console::Console::new();
    let mut writer_guard = unaos_kernel::writer::WRITER.lock();
    let mut pal = unaos_kernel::pal::TargetPal::new(&mut *writer_guard);
    
    console.draw(&mut pal);

    loop {
        use unaos_kernel::pal::GneissPal;
        let event = pal.poll_event();
        match event {
            unaos_kernel::pal::Event::Key(c) => {
                if c == b'\n' || c == b'\r' {
                    let cmd = console.current_input.clone();
                    console.current_input.clear();
                    unaos_kernel::shell::dispatch_command(&cmd, &mut console, &mut pal);
                    console.draw(&mut pal);
                } else if c == 8 || c == 0x7F {
                    console.current_input.pop();
                    console.draw(&mut pal);
                } else if c >= 32 && c <= 126 {
                    console.current_input.push(c as char);
                    console.draw(&mut pal);
                }
            }
            _ => {}
        }
        unaos_kernel::arch::hlt_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial_println!("{}", info);
    unaos_kernel::arch::hlt_loop();
}
