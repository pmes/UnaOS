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

// The `bootlog` feature halts before the GUI, making the GUI/main-loop code below unreachable.
#[cfg_attr(feature = "bootlog", allow(unreachable_code))]
fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    // 0. Framebuffer log sink FIRST — mirror every serial_println! (and panics) to the screen,
    //    so boot diagnostics are visible on real hardware that has no serial port. No-op if the
    //    firmware gave us no framebuffer. The GUI repaints over it later on a successful boot.
    unaos_kernel::video::fbcon::init(
        boot_info.framebuffer_addr,
        boot_info.framebuffer_size,
        boot_info.framebuffer_info,
    );

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

    // 4. Global Heap Allocation (Phase 3 Memory Translation)
    unaos_kernel::arch::memory::init(boot_info);
    serial_println!(":: KERNEL HEAP ALLOCATED ::");

    // 5. Motherboard Hardware Interconnects (xHCI/USB bring-up).
    //    Skippable via the `skip_xhci` Cargo feature (UNAOS_SKIP_XHCI=1) so the video stack still
    //    comes up promptly on real hardware where firmware/SMM may still own the xHCI controller
    //    (no BIOS->OS handoff on this branch) and never reflect our reset writes — which would
    //    otherwise stall boot in the bounded timeout loops before the first GUI frame paints.
    #[cfg(not(feature = "skip_xhci"))]
    unaos_kernel::arch::pci::init(dtb_addr, dtb_size);
    #[cfg(feature = "skip_xhci")]
    {
        let _ = (dtb_addr, dtb_size);
        serial_println!(":: xHCI bring-up SKIPPED (skip_xhci feature): video only, no USB ::");
    }

    // Boot-log mode: hold the fbcon boot log on screen (no GUI takeover, no background paint) so
    // it can be photographed on serial-less hardware. Dump the effective framebuffer geometry and
    // pixel format — i.e. the result of the bootloader's EDID/GOP mode selection — then halt.
    #[cfg(feature = "bootlog")]
    {
        let fmt = match info.pixel_format {
            unaos_boot_info::PixelFormat::Rgb => "Rgb",
            unaos_boot_info::PixelFormat::Bgr => "Bgr",
            unaos_boot_info::PixelFormat::U8 => "U8",
            _ => "Unknown",
        };
        serial_println!(":: ============== BOOT LOG MODE ============== ::");
        serial_println!(
            ":: framebuffer {}x{}  stride={}px  bpp={}  fmt={} ::",
            info.width, info.height, info.stride, info.bytes_per_pixel, fmt
        );
        serial_println!(
            ":: fb_addr={:#x}  fb_size={}  stride*h*bpp={} ::",
            framebuffer_addr,
            framebuffer_size,
            info.stride * info.height * info.bytes_per_pixel
        );
        serial_println!(":: GUI suppressed; boot log held on screen. Power off when done. ::");
        unaos_kernel::arch::hlt_loop();
    }

    if framebuffer_addr != 0 {
        // Safety: the bootloader passed a valid, identity-mapped framebuffer base address
        // (physical_memory_offset == 0). The video surface addresses it directly.
        unaos_kernel::video::WRITER
            .lock()
            .init(framebuffer_addr as usize, framebuffer_size, info);

        unaos_kernel::vug::init(framebuffer_addr as usize, framebuffer_size, info);
    } else {
        serial_println!(":: WARNING: No framebuffer detected ::");
    }

    let mut console = unaos_kernel::console::Console::new();

    // Build the double-buffered screen over the framebuffer. FrameBuffer is Copy, so we take a
    // handle and release the WRITER lock immediately. All GUI drawing now goes to a cached-RAM
    // back buffer; render() flushes only the damaged region to the (slow) framebuffer.
    let front_fb = *unaos_kernel::video::WRITER.lock();
    let mut screen = unaos_kernel::video::Screen::new(front_fb);
    let mut pal = unaos_kernel::pal::TargetPal::new(&mut screen);

    console.draw(&mut pal);
    pal.render();

    // The GUI now owns the screen — stop fbcon mirroring serial output onto the framebuffer
    // (a panic re-enables it). Boot diagnostics up to this first frame stay on screen until now.
    unaos_kernel::video::fbcon::detach();

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
                    // A command may take over the whole screen (e.g. `vug`); don't repaint the
                    // console over it in that case — leave it up until the next keypress.
                    let took_screen =
                        unaos_kernel::shell::dispatch_command(&cmd, &mut console, &mut pal);
                    if !took_screen {
                        console.draw(&mut pal);
                    }
                } else if c == 8 || c == 0x7F {
                    console.current_input.pop();
                    console.draw_input_line(&mut pal);
                } else if c >= 32 && c <= 126 {
                    console.current_input.push(c as char);
                    console.draw_input_line(&mut pal);
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

        // Present this frame: flush the damaged region of the back buffer to the framebuffer.
        // No-op when nothing was drawn this iteration, so the idle (hlt) path stays cheap.
        pal.render();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Paint a red panic backdrop on the framebuffer (visible on hardware with no serial), then
    // print the message — serial_println! mirrors it onto that backdrop via fbcon.
    unaos_kernel::video::fbcon::panic_screen();
    serial_println!("=== KERNEL PANIC ===");
    serial_println!("{}", info);
    unaos_kernel::arch::hlt_loop();
}
