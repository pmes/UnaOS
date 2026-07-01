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

// UEFI aarch64 entry (default): the bootloader hands us a BootInfo with the MMU already on.
#[unsafe(no_mangle)]
#[cfg(all(target_arch = "aarch64", not(feature = "baremetal")))]
pub extern "C" fn _start(boot_info: &'static mut BootInfo) -> ! {
    kernel_main(boot_info)
}

// Bare-metal aarch64 entry (`baremetal` feature): the Raspberry Pi GPU ROM loads our flat
// kernel8.img to 0x80000 and jumps to `_start` at EL2 with x0 = DTB pointer, MMU off, no stack.
// `.text.boot` is placed first by pi-baremetal.ld so `_start` is at the load address. It parks
// secondary cores (the firmware starts only core 0 at the kernel, but guard anyway), zeroes BSS,
// sets SP to the linker-reserved stack, and tail-calls `__rust_boot` with the DTB pointer.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
core::arch::global_asm!(
    r#"
    .section .text.boot, "ax", %progbits
    .globl _start
_start:
    mrs   x1, mpidr_el1
    and   x1, x1, #0xff          // Aff0 = core id (Pi 4 is a single cluster)
    cbnz  x1, .Lpark             // only core 0 proceeds
    mov   x19, x0                // save the DTB pointer across the BSS clear
    // zero BSS: [__bss_start, __bss_end)
    adrp  x0, __bss_start
    add   x0, x0, #:lo12:__bss_start
    adrp  x2, __bss_end
    add   x2, x2, #:lo12:__bss_end
.Lbss:
    cmp   x0, x2
    b.hs  .Lstack
    str   xzr, [x0], #8
    b     .Lbss
.Lstack:
    adrp  x0, __stack_top
    add   x0, x0, #:lo12:__stack_top
    mov   sp, x0
    mov   x0, x19                // DTB pointer as the first argument
    bl    __rust_boot
.Lpark:
    wfe
    b     .Lpark
"#
);

#[unsafe(no_mangle)]
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
pub extern "C" fn __rust_boot(dtb: u64) -> ! {
    // SP is set and BSS is zeroed (by _start). Enable the MMU before anything touches a lock/atomic,
    // then synthesize the BootInfo UEFI would normally provide and enter the shared kernel path.
    unsafe { unaos_kernel::arch::boot::mmu_init() };
    let boot_info = unaos_kernel::arch::boot::build_boot_info(dtb);
    kernel_main(boot_info)
}

// The `bootlog` feature halts before the GUI, and `baremetal` enters a serial-only loop instead —
// both make the GUI/main-loop code below unreachable.
#[cfg_attr(feature = "bootlog", allow(unreachable_code))]
#[cfg_attr(feature = "baremetal", allow(unreachable_code))]
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

    // ACPI RSDP (x86_64) before memory init consumes boot_info
    #[cfg(target_arch = "x86_64")]
    let rsdp_addr = boot_info.rsdp_addr;

    // EDID/mode-selection diagnostics (read before memory::init consumes boot_info); only the
    // bootlog build uses them, so gate the extraction to avoid unused-field warnings elsewhere.
    #[cfg(feature = "bootlog")]
    let (edid_native_w, edid_native_h, edid_source, mode_action) = (
        boot_info.edid_native_width,
        boot_info.edid_native_height,
        boot_info.edid_source,
        boot_info.mode_action,
    );

    // 4. Global Heap Allocation (Phase 3 Memory Translation)
    unaos_kernel::arch::memory::init(boot_info);
    serial_println!(":: KERNEL HEAP ALLOCATED ::");

    // 4a. aarch64 SMP (bare-metal Pi 4): release the 3 parked Cortex-A72 secondary cores from the
    // firmware spin-table. Each brings up its own MMU + exception vectors and (Milestone 1) reports
    // in over serial, then idles. The BSP continues below as the hardware-service core.
    #[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
    unaos_kernel::arch::smp::start_secondaries();

    // 4b. aarch64 scheduler (M3a): a cooperative round-robin smoke test on the boot core — spawn a
    // few kernel threads that yield to each other and exit, proving the context switch + run queue.
    // No interrupts required, so it runs in QEMU too. Runs BEFORE preemption is on (stays cooperative).
    #[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
    unaos_kernel::arch::sched::demo_cooperative();

    // 4c. aarch64 scheduler (M3b): turn on preemption and put a workload on the secondary cores.
    // On metal each AP's tasks are timer-preempted (they interleave); in QEMU there is no Group-1
    // delivery, so the APs run their tasks to completion sequentially. The BSP is never scheduled —
    // it continues below as the GUI/hardware-service core.
    #[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
    let input_on_scheduler = {
        let online = unaos_kernel::arch::smp::online_secondaries();
        unaos_kernel::arch::sched::start_aps(&online);
        // M5: hand keyboard input to a scheduled kernel thread on a secondary core, so the main loop
        // below only drains events + renders instead of polling the UART itself — the OS servicing
        // its own input via the scheduler. Only on the GUI path: the serial-only fallback (no
        // framebuffer) echoes input on the BSP, and two readers of the one UART would steal each
        // other's bytes. Needs an online AP to host it; if none came up, the BSP keeps polling.
        let host = if framebuffer_addr != 0 { online.last().copied() } else { None };
        if let Some(cpu) = host {
            unaos_kernel::arch::sched::spawn("input", input_service, 0, cpu);
            serial_println!(":: INPUT: keyboard service scheduled on core {} (BSP stops polling) ::", cpu);
            true
        } else {
            false
        }
    };
    // aarch64-UEFI (and any non-baremetal aarch64) has no scheduler: the BSP polls input itself.
    #[cfg(all(target_arch = "aarch64", not(feature = "baremetal")))]
    let input_on_scheduler = false;
    // `bootlog` halts (hlt_loop) before the GUI main loop that reads this flag, so its only reader is
    // unreachable in that build — acknowledge the binding here (reachable, before the halt) to avoid
    // an unused-variable warning without weakening the flag on the normal GUI path.
    #[cfg(all(target_arch = "aarch64", feature = "bootlog"))]
    let _ = input_on_scheduler;

    // 4b. ACPI: discover the CPU topology (MADT) for SMP bring-up. x86_64 only — aarch64
    // discovers CPUs via the DTB. Degrades gracefully to uniprocessor if ACPI is absent.
    #[cfg(target_arch = "x86_64")]
    unaos_kernel::arch::acpi::init(rsdp_addr);

    // 4c. SMP: start the application processors (INIT-SIPI-SIPI). Each AP brings up its own
    // per-CPU GDT/TSS + local APIC, then waits to enter its scheduler loop; the BSP continues to
    // drive everything below. `start_aps` also runs the post-bring-up SMP smoke test while the
    // APs are still idle.
    #[cfg(target_arch = "x86_64")]
    unaos_kernel::arch::smp::start_aps();

    // 4d. Scheduler: now that SMP verification has run against idle APs, initialise the per-CPU
    // run queues, turn scheduling on, and spawn a small demo workload across the APs to exercise
    // preemption / cooperative yield / task exit. The BSP itself is never scheduled — it stays
    // the hardware-service core in the loop below.
    #[cfg(target_arch = "x86_64")]
    {
        unaos_kernel::arch::sched::init();
        let online = unaos_kernel::arch::smp::online_aps();
        unaos_kernel::arch::sched::start_demo(&online);
    }

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
        let edid_src = match edid_source {
            1 => "ACTIVE-protocol",
            2 => "DISCOVERED-protocol",
            _ => "none",
        };
        let action = match mode_action {
            1 => "set EDID-native mode",
            2 => "set fallback linear mode",
            3 => "headless (no linear fb)",
            _ => "kept firmware current mode",
        };
        serial_println!(":: EDID read: source={}  native={}x{} ::", edid_src, edid_native_w, edid_native_h);
        serial_println!(":: mode selection: {} ::", action);
        serial_println!(":: GUI suppressed; boot log held on screen. Power off when done. ::");
        unaos_kernel::arch::hlt_loop();
    }

    // Bare-metal Pi 4: booted straight from the microSD slot via the GPU ROM, no UEFI. Phase 2 asks
    // the VideoCore GPU for a framebuffer over the mailbox (in build_boot_info), so on a Pi with HDMI
    // `framebuffer_addr` is now non-zero and we fall through to the full GUI path below — the PL011
    // serves as the keyboard (the main loop polls it on aarch64). If the mailbox allocation failed
    // (or a headless config), fall back to the Phase-1 serial-only console: arch::init already
    // streamed EL/GIC/timer-LIVE to the PL011, so this just echoes input. The timer heartbeat keeps
    // WFI waking, so input is serviced promptly.
    #[cfg(feature = "baremetal")]
    if framebuffer_addr == 0 {
        let _ = (framebuffer_size, info); // unused on the serial-only path
        serial_println!(":: UnaOS bare-metal — Pi 4 microSD-slot boot, serial console (no framebuffer) ::");
        serial_println!(":: heartbeat live; type and I echo. ::");
        loop {
            while let Some(b) = unaos_kernel::arch::poll_input() {
                // Echo; map CR to CRLF so a serial terminal advances lines.
                if b == b'\r' {
                    unaos_kernel::serial_print!("\r\n");
                } else {
                    unaos_kernel::serial_print!("{}", b as char);
                }
            }
            unaos_kernel::hlt();
        }
    } else {
        serial_println!(":: UnaOS bare-metal — Pi 4, VideoCore framebuffer up; starting GUI ::");
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
            xhci.service_hubs();
        }

        // Drain any frames the NIC has received into the network stack (no-op when
        // no NIC is present, e.g. on aarch64).
        unaos_kernel::drivers::e1000::service_net();

        // aarch64: unless a scheduled input service owns the UART (M5), poll it here and feed the
        // event queue. Drain all pending bytes so a burst isn't spread one-per-frame.
        #[cfg(target_arch = "aarch64")]
        if !input_on_scheduler {
            while let Some(byte) = unaos_kernel::arch::poll_input() {
                unaos_kernel::pal::push_event(unaos_kernel::pal::Event::Key(byte));
            }
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

/// M5 (bare-metal aarch64): keyboard input as a scheduled kernel service.
///
/// This is the first step of "the OS runs on its own scheduler" — instead of the BSP polling the
/// PL011 inline in its GUI loop, keyboard input is a kernel thread on a secondary core. It drains
/// every byte waiting in the UART RX FIFO into the shared `pal` event queue (a cross-core
/// `spin::Mutex`), which the BSP's render loop pops and dispatches. Only the byte *source* moves off
/// the BSP; the console behaves identically. Never returns (a service task).
///
/// Between polls it naps so it does not pin its core: on metal the per-core generic-timer tick wakes
/// it every tick (~4 ms at 250 Hz — imperceptible for typing), and the core WFI-idles in between. In
/// QEMU raspi4b the timer IRQ is not delivered, so `sleep_ticks` would park forever (`is_live()` is
/// false there); yield cooperatively instead so this core's `run()` keeps re-dispatching us.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn input_service(_: usize) {
    loop {
        while let Some(byte) = unaos_kernel::arch::poll_input() {
            unaos_kernel::pal::push_event(unaos_kernel::pal::Event::Key(byte));
        }
        if unaos_kernel::arch::timer::is_live() {
            unaos_kernel::arch::sched::sleep_ticks(1);
        } else {
            unaos_kernel::arch::sched::yield_now();
        }
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
