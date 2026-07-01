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

// `bootlog` halts before the GUI, `usbdebug` loops forever before it, and `baremetal` enters a
// serial-only loop (or hands the GUI to scheduled tasks) instead — all make the GUI/main-loop code
// below unreachable in those builds.
#[cfg_attr(any(feature = "bootlog", feature = "usbdebug", feature = "baremetal"), allow(unreachable_code))]
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
    {
        let online = unaos_kernel::arch::smp::online_secondaries();
        unaos_kernel::arch::sched::start_aps(&online);
    }

    // 4b. ACPI: discover the CPU topology (MADT) for SMP bring-up. x86_64 only — aarch64
    // discovers CPUs via the DTB. Degrades gracefully to uniprocessor if ACPI is absent.
    #[cfg(target_arch = "x86_64")]
    unaos_kernel::arch::acpi::init(rsdp_addr);

    // 4b'. VT-d / IOMMU check (F5): the kernel DMAs untranslated, identity-mapped heap buffers to
    // xHCI/e1000. If firmware has DMA remapping ENABLED, that DMA is blocked — report it before USB
    // bring-up so a metal boot sees the cause instead of a silent xHCI failure.
    #[cfg(target_arch = "x86_64")]
    unaos_kernel::arch::acpi::dmar_report(rsdp_addr);

    // 4b''. Timebase reference: prove the ACPI PM timer (fixed 3.579545 MHz) is live before we
    // calibrate the TSC / APIC timer against it. On a serial-less laptop this line is the evidence
    // the calibration clock works; "STUCK?" or "not found" means the timebase stays uncalibrated.
    #[cfg(target_arch = "x86_64")]
    unaos_kernel::arch::acpi::pm_timer_report(rsdp_addr);

    // 4b'''. Calibrate the TSC and the local-APIC timer against the PM timer, so tick-based timing
    // (scheduler sleeps, net RTO) and cycle-based busy-wait budgets become real wall-clock on this
    // machine's unknown Ivy Bridge crystal. Must precede SMP/scheduler bring-up so the APs inherit
    // the calibrated timer. No-op (fixed fallbacks) if the PM timer is absent.
    #[cfg(target_arch = "x86_64")]
    if let Some(pm) = unaos_kernel::arch::acpi::pm_timer(rsdp_addr) {
        unaos_kernel::arch::apic::calibrate(&pm);
    }

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
        // The demo workload (incl. the RwLock self-test) uses tick-based timing. The APIC timer is
        // now calibrated to a real 1 kHz (see step 4b'''), so it runs at normal speed on metal —
        // no more multi-minute stall. It's still just a QEMU-verified smoke test, so keep it opt-in
        // (UNAOS_SCHED_DEMO=1 -> `sched_demo` feature); by default the scheduler initializes but no
        // demo threads spawn. Never in usbdebug.
        #[cfg(all(feature = "sched_demo", not(feature = "usbdebug")))]
        {
            let online = unaos_kernel::arch::smp::online_aps();
            unaos_kernel::arch::sched::start_demo(&online);
        }
    }

    // 4e. Prove the global ms-clock is real: with every core now online and ticking at 1 kHz, the
    // shared `ticks()` clock must still advance at ~1000 Hz (only the BSP drives it). This is the
    // wall-clock assertion the calibration hinges on — a reading of ~N×1000 would betray an SMP
    // over-count. Surfaced on the framebuffer for the serial-less metal boot.
    #[cfg(target_arch = "x86_64")]
    if let Some(pm) = unaos_kernel::arch::acpi::pm_timer(rsdp_addr) {
        unaos_kernel::arch::apic::report_tick_rate(&pm);
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
    // `framebuffer_addr` is now non-zero and we fall through to the GUI path below (which, with APs
    // online, is handed to the scheduled input+render tasks; the BSP idles). If the mailbox
    // allocation failed (or a headless config), fall back to the Phase-1 serial-only console.
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

    // USB bring-up debug view (serial-less hardware): keep the boot log on the framebuffer (no GUI
    // takeover, no fbcon detach) and run the full main-loop USB path, printing each input event.
    // So external USB storage/keyboard/mouse enumeration AND live input are visible + photographable
    // on metal. (Net service is intentionally skipped here so a non-e1000 NIC isn't poked.)
    #[cfg(feature = "usbdebug")]
    {
        // Clear the boot spam so the (post-boot) hot-plug enumeration + live input own the screen.
        unaos_kernel::video::fbcon::clear();
        serial_println!(":: ============== USB DEBUG MODE ============== ::");
        serial_println!(":: Enumerating USB. Plug in a stick / keyboard / mouse, then type or move the mouse. ::");
        serial_println!(":: Watch for: 'MISSION SUCCESS' (storage), 'POINTER ... ABSOLUTE/RELATIVE', 'KEY', and the USB-DEBUG lines below. ::");
        loop {
            if let Some(xhci) = &mut *unaos_kernel::drivers::xhci::XHCI_CONTROLLER.lock() {
                xhci.poll_events();
                xhci.service_storage();
                xhci.service_hubs();
                xhci.service_hid_setproto();
            }
            while let Some(event) = unaos_kernel::pal::next_event() {
                match event {
                    unaos_kernel::pal::Event::Key(c) => {
                        let ch = c as char;
                        serial_println!("USB-DEBUG: KEY {:#04x} '{}'", c, if c >= 32 && c < 127 { ch } else { '.' });
                    }
                    unaos_kernel::pal::Event::Mouse { x, y } => {
                        serial_println!("USB-DEBUG: MOUSE relative dx={} dy={}", x, y);
                    }
                    unaos_kernel::pal::Event::MouseAbsolute { x, y } => {
                        serial_println!("USB-DEBUG: MOUSE absolute x={} y={}", x, y);
                    }
                    _ => {}
                }
            }
            unaos_kernel::hlt();
        }
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

    // M5 (bare-metal aarch64): run the interactive OS on its own scheduler. Keyboard input and GUI
    // render become scheduled kernel threads on secondary cores, communicating over GUI_CHANNEL; the
    // BSP hands the framebuffer off (globals set above) and idles. Spawn BOTH here, together and only
    // once the framebuffer is ready, so the input producer never runs without its render consumer
    // (else a keystroke burst could fill the channel, block send(), and stall UART draining). Host
    // them on DIFFERENT APs (render on online.first(), input on online.last()) so the Channel
    // send/recv wakes cross-core — the metal-only path; with a single AP they coincide and cooperate.
    // Detach fbcon HERE (before the render task paints) so exactly one core writes the framebuffer.
    // If no AP came up (or the serial-only fallback took the early return), fall through to the shared
    // BSP loop below, which polls input + renders itself.
    #[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
    if framebuffer_addr != 0 {
        let online = unaos_kernel::arch::smp::online_secondaries();
        if let (Some(&render_cpu), Some(&input_cpu)) = (online.first(), online.last()) {
            GUI_CHANNEL.init(); // reserve waiter capacity on the BSP before the tasks block on it
            unaos_kernel::arch::serial::RX_READY.init(); // M5c: the RX-wake semaphore's waiter list
            unaos_kernel::video::fbcon::detach();
            // M5c: on metal, route + enable the PL011 RX interrupt (SPI 153) to the input core so the
            // input task is woken by the UART instead of polling. GICD config stays BSP-only (this is
            // global distributor state). A backstop task also periodically wakes the input service so
            // input still works (degraded to polling) if the SPI never delivers on some board. QEMU
            // raspi4b delivers no Group-1 IRQ (is_live() false) → skip both; the input task polls.
            if unaos_kernel::arch::timer::is_live() {
                unaos_kernel::arch::gic::enable_spi(
                    unaos_kernel::arch::serial::PL011_RX_INTID,
                    input_cpu,
                );
                unaos_kernel::arch::sched::spawn("rx-backstop", rx_backstop, 0, input_cpu);
            }
            unaos_kernel::arch::sched::spawn("input", input_service, 0, input_cpu);
            unaos_kernel::arch::sched::spawn("render", render_service, 0, render_cpu);
            serial_println!(
                ":: INPUT on core {} + RENDER on core {} scheduled (OS on its own scheduler; BSP idle) ::",
                input_cpu, render_cpu
            );
            unaos_kernel::arch::hlt_loop();
        }
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
            xhci.service_hid_setproto();
        }

        // Drain any frames the NIC has received into the network stack (no-op when
        // no NIC is present, e.g. on aarch64).
        unaos_kernel::drivers::e1000::service_net();

        // aarch64 (UEFI, or the bare-metal no-AP fallback): poll the UART here and feed the event
        // queue, draining all pending bytes so a burst isn't spread one-per-frame. On bare-metal with
        // APs this loop is never reached — the scheduled input+render services own input and the BSP
        // idles above — so no two-readers-of-one-UART hazard.
        #[cfg(target_arch = "aarch64")]
        while let Some(byte) = unaos_kernel::arch::poll_input() {
            unaos_kernel::pal::push_event(unaos_kernel::pal::Event::Key(byte));
        }

        // Drain ALL queued input events this iteration, then present ONE frame below. A burst of
        // mouse-move reports (or fast typing) must not back up one-event-per-iteration behind the
        // framebuffer flush — at native resolution that flush is slow, so processing a single event
        // per loop made input lag badly (the cursor never caught up; typed text appeared seconds
        // late). Apply every pending event to the back buffer here; `render()` coalesces them.
        let mut had_event = false;
        loop {
            match pal.poll_event() {
                unaos_kernel::pal::Event::None => break,
                unaos_kernel::pal::Event::Key(c) => {
                    had_event = true;
                    // `handle_key` returns true if the command took over the whole screen (e.g.
                    // `vug`); stop draining this frame so a keystroke already queued behind Enter
                    // can't paint the console back over the full-screen output — present it alone,
                    // handle the rest next frame. (Shared with the scheduled render service.)
                    if handle_key(c, &mut console, &mut pal) {
                        break;
                    }
                }
                unaos_kernel::pal::Event::Mouse { x, y } => {
                    had_event = true;
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
                    had_event = true;
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
                // Timer / Unknown: nothing to do.
                _ => {}
            }
        }

        // Nothing queued — sleep until the next interrupt (timer/xHCI) rather than busy-spin.
        if !had_event {
            unaos_kernel::hlt();
        }

        // Present this frame: flush the damaged region of the back buffer to the framebuffer.
        // No-op when nothing was drawn this iteration, so the idle (hlt) path stays cheap.
        pal.render();
    }
}

/// Handle one keyboard byte against the console: printable ASCII extends the input line, backspace
/// (BS/DEL) erases, and CR/LF dispatches the line as a shell command. Returns `true` iff the command
/// took over the whole screen (e.g. `vug`) — in which case the console is NOT repainted over it, and
/// a drain-loop caller should stop draining this frame so a queued keystroke can't paint the console
/// back over the full-screen output. Shared by the BSP GUI loop (x86 / aarch64-UEFI / the no-AP
/// fallback) and the scheduled render service, so both drive the console identically.
fn handle_key(
    c: u8,
    console: &mut unaos_kernel::console::Console,
    pal: &mut unaos_kernel::pal::TargetPal<'_>,
) -> bool {
    if c == b'\n' || c == b'\r' {
        let cmd = console.current_input.clone();
        console.current_input.clear();
        let took_screen = unaos_kernel::shell::dispatch_command(&cmd, console, pal);
        if !took_screen {
            console.draw(pal);
        }
        return took_screen;
    } else if c == 8 || c == 0x7F {
        console.current_input.pop();
        console.draw_input_line(pal);
    } else if c >= 32 && c <= 126 {
        console.current_input.push(c as char);
        console.draw_input_line(pal);
    }
    false
}

/// M5b: the keyboard-event channel from the input service to the render service (bare-metal aarch64).
/// The input thread `send`s Key events; the render thread `recv`s them — a cross-core handoff (the two
/// run on different APs), dogfooding the M4 `Channel`. Capacity 64 matches the old event ring; a full
/// channel applies backpressure to the input thread rather than dropping keystrokes.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static GUI_CHANNEL: unaos_kernel::arch::sched::Channel<unaos_kernel::pal::Event> =
    unaos_kernel::arch::sched::Channel::new(64);

/// One-shot guard: log "RX interrupt live" exactly once, from the input task (never the ISR).
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static RX_LOGGED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// M5 (bare-metal aarch64): keyboard input as a scheduled kernel service. The OS runs its own input
/// on the scheduler: instead of the BSP polling the PL011 inline, this kernel thread on a secondary
/// core drains bytes from the UART RX FIFO and `send`s each as a Key event over GUI_CHANNEL to the
/// render service (M5b). Never returns (a service task).
///
/// M5c: on metal it is INTERRUPT-DRIVEN — the PL011 RX interrupt (routed to this core by the BSP)
/// wakes it via `serial::RX_READY`, so the core WFI-idles until a keystroke instead of tick-polling.
/// In QEMU raspi4b no Group-1 IRQ is delivered (`is_live()` false), so it falls back to a cooperative
/// poll loop (the RX ISR never fires there). The two paths differ only in how the drain is woken.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn input_service(_: usize) {
    use core::sync::atomic::Ordering;
    use unaos_kernel::arch::serial;

    if unaos_kernel::arch::timer::is_live() {
        // Interrupt-driven (metal). The BSP already enabled + routed the GIC SPI to this core; arm
        // the PL011's own RX interrupts, then block until the ISR posts RX_READY.
        serial::enable_rx_interrupt();
        loop {
            assert!(serial::RX_READY.wait(), "input service ran off a scheduled task");
            // Confirm (once, off the ISR) that a real RX interrupt actually fired on this board —
            // distinguishes an interrupt wake from a backstop poll.
            if serial::RX_IRQ_SEEN.load(Ordering::Relaxed) && !RX_LOGGED.swap(true, Ordering::Relaxed)
            {
                serial_println!(":: INPUT: PL011 RX interrupt live — keyboard is interrupt-driven ::");
            }
            while let Some(byte) = unaos_kernel::arch::poll_input() {
                GUI_CHANNEL.send(unaos_kernel::pal::Event::Key(byte));
            }
            serial::rearm_rx_interrupt(); // re-enable IMSC (no ICR — keeps a straggler's timeout)
            // Close the drain/re-arm gap: if a byte landed meanwhile, wake ourselves to drain it
            // rather than wait for the next receive-timeout.
            if serial::rx_pending() {
                serial::RX_READY.post();
            }
        }
    } else {
        // Poll-nap fallback (QEMU raspi4b: the RX ISR never fires). Cooperative — the AP's run() keeps
        // re-dispatching us; sleep_ticks would park forever with no timer IRQ to wake it.
        loop {
            while let Some(byte) = unaos_kernel::arch::poll_input() {
                GUI_CHANNEL.send(unaos_kernel::pal::Event::Key(byte));
            }
            unaos_kernel::arch::sched::yield_now();
        }
    }
}

/// M5c liveness backstop (metal only): periodically wake the input service so keyboard input keeps
/// working — degraded to ~200 ms polling — even if the PL011 RX interrupt never delivers on some
/// board. On a working GIC the RX ISR wakes the input task at interrupt latency and this just
/// redundantly pokes an empty FIFO (cheap). Never returns.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn rx_backstop(_: usize) {
    loop {
        unaos_kernel::arch::sched::sleep_ticks(50); // ~200 ms at the 250 Hz per-core tick
        unaos_kernel::arch::serial::RX_READY.post();
    }
}

/// M5b (bare-metal aarch64): the GUI/render service — the interactive OS as a scheduled kernel task.
///
/// Runs on a secondary core (NOT the BSP): builds the double-buffered `Screen` + `Console` over the
/// framebuffer the BSP initialised in `WRITER` (and detached fbcon from), paints the first frame, then
/// blocks on GUI_CHANNEL for keyboard events from the input service (a cross-core `recv`) and
/// dispatches each through the shared `handle_key`, presenting the damaged region after each. Never
/// returns. Together with `input_service` this is "the OS runs on its own scheduler": input + render
/// are scheduled kernel threads communicating over a Channel, and the BSP is freed from the GUI loop.
///
/// Blocking on `recv` (vs a poll-nap) means whenever there is no input this task is off the run queue
/// entirely and its core WFI-idles; it wakes only when the input service sends, via the reschedule SGI.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn render_service(_: usize) {
    use unaos_kernel::pal::GneissPal; // for pal.render()
    // FrameBuffer is Copy: take a handle and build the back-buffered surface. All drawing goes to
    // cached RAM; render() flushes only the damaged span to the framebuffer, cleaning the cache so
    // the (non-snooping) VideoCore scan-out sees it.
    let front_fb = *unaos_kernel::video::WRITER.lock();
    let mut screen = unaos_kernel::video::Screen::new(front_fb);
    let mut pal = unaos_kernel::pal::TargetPal::new(&mut screen);
    let mut console = unaos_kernel::console::Console::new();

    console.draw(&mut pal);
    pal.render();

    loop {
        if let unaos_kernel::pal::Event::Key(c) = GUI_CHANNEL.recv() {
            handle_key(c, &mut console, &mut pal);
        }
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
