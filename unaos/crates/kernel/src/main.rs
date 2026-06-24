#![no_std]
#![no_main]
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

extern crate alloc;

use core::panic::PanicInfo;
use unaos_kernel::serial_println;
use unaos_boot_info::BootInfo;

#[unsafe(no_mangle)]
#[cfg(target_arch = "x86_64")]
pub extern "sysv64" fn _start(boot_info: &'static mut BootInfo) -> ! {
    kernel_main(boot_info)
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "aarch64")]
pub extern "C" fn _start(boot_info: &'static mut BootInfo) -> ! {
    kernel_main(boot_info)
}

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    // 1. Core Hardware Init (GDT, IDT, local APIC for x86_64, GIC for aarch64)
    unaos_kernel::init();

    // 3. Framebuffer Info Extraction
    // Extract info before memory initialization consumes the BootInfo reference
    let framebuffer_addr = boot_info.framebuffer_addr;
    let framebuffer_size = boot_info.framebuffer_size;
    let info = boot_info.framebuffer_info;

    // Extract DTB info before memory init consumes boot_info
    let dtb_addr = boot_info.dtb_addr;
    let dtb_size = boot_info.dtb_size;

    // ACPI RSDP (x86_64) before memory init consumes boot_info
    #[cfg(target_arch = "x86_64")]
    let rsdp_addr = boot_info.rsdp_addr;

    // 4. Global Heap Allocation (Phase 3 Memory Translation)
    unaos_kernel::arch::memory::init(boot_info);
    serial_println!(":: KERNEL HEAP ALLOCATED ::");

    // 4b. ACPI: discover the CPU topology (MADT) for SMP bring-up. x86_64 only — aarch64
    // discovers CPUs via the DTB. Degrades gracefully to uniprocessor if ACPI is absent.
    #[cfg(target_arch = "x86_64")]
    unaos_kernel::arch::acpi::init(rsdp_addr);

    // 4c. SMP: start the application processors (INIT-SIPI-SIPI). Each AP brings up its own
    // per-CPU GDT/TSS + local APIC and idles; the BSP continues to drive everything below.
    #[cfg(target_arch = "x86_64")]
    unaos_kernel::arch::smp::start_aps();

    // 5. Motherboard Hardware Interconnects
    unaos_kernel::arch::pci::init(dtb_addr, dtb_size);

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

    use unaos_kernel::pal::GneissPal;
    let mut mouse_px: i32 = (pal.width() / 2) as i32;
    let mut mouse_py: i32 = (pal.height() / 2) as i32;

    loop {
        // Poll xHCI Controller, then run any deferred storage work (synchronous BOT
        // transactions run here, in a safe non-event context).
        if let Some(xhci) = &mut *unaos_kernel::drivers::xhci::XHCI_CONTROLLER.lock() {
            xhci.poll_events();
            xhci.service_storage();
        }

        #[cfg(target_arch = "aarch64")]
        if let Some(byte) = unaos_kernel::arch::poll_input() {
            unaos_kernel::pal::push_event(unaos_kernel::pal::Event::Key(byte));
        }

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
            unaos_kernel::pal::Event::Mouse { x, y } => {
                // Erase old cursor (draw background color over it)
                pal.draw_rect(mouse_px as usize, mouse_py as usize, 10, 10, 0x1E1E1E);

                // Update position with deltas
                mouse_px += x;
                mouse_py += y;

                // Clamp to screen bounds
                if mouse_px < 0 { mouse_px = 0; }
                if mouse_py < 0 { mouse_py = 0; }
                if mouse_px as u32 >= pal.width() { mouse_px = pal.width() as i32 - 10; }
                if mouse_py as u32 >= pal.height() { mouse_py = pal.height() as i32 - 10; }

                // Draw new cursor (a bright red 10x10 square)
                pal.draw_rect(mouse_px as usize, mouse_py as usize, 10, 10, 0xFF0000);
            }
            unaos_kernel::pal::Event::MouseAbsolute { x, y } => {
                // Erase old cursor
                pal.draw_rect(mouse_px as usize, mouse_py as usize, 10, 10, 0x1E1E1E);

                // Scale 0-32767 coordinate space to screen bounds
                mouse_px = ((x as i64 * pal.width() as i64) / 32767) as i32;
                mouse_py = ((y as i64 * pal.height() as i64) / 32767) as i32;

                // Clamp just in case
                if mouse_px < 0 { mouse_px = 0; }
                if mouse_py < 0 { mouse_py = 0; }
                if mouse_px as u32 >= pal.width() { mouse_px = pal.width() as i32 - 10; }
                if mouse_py as u32 >= pal.height() { mouse_py = pal.height() as i32 - 10; }

                // Draw new cursor
                pal.draw_rect(mouse_px as usize, mouse_py as usize, 10, 10, 0xFF0000);
            }
            _ => {
                unaos_kernel::hlt();
            }
        }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial_println!("{}", info);
    unaos_kernel::arch::hlt_loop();
}
