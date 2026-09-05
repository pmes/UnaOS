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
    // SP is set and BSS is zeroed (by _start). The firmware handed us off at EL2; drop the boot core
    // to EL1 (the standard OS level) BEFORE the MMU, so enable_mmu and every lock/atomic below run in
    // the EL1&0 regime on Normal-cacheable memory. Then enable the MMU, synthesize the BootInfo UEFI
    // would normally provide, and enter the shared kernel path.
    unsafe { unaos_kernel::arch::boot::drop_to_el1() };
    unsafe { unaos_kernel::arch::boot::mmu_init() };
    let boot_info = unaos_kernel::arch::boot::build_boot_info(dtb);
    kernel_main(boot_info)
}

// `bootlog` halts before the GUI, `usbdebug` loops forever before it, `baremetal` enters a
// serial-only loop (or hands the GUI to scheduled tasks) instead, and `tegra` stops at the early
// platform stop below (Jetson Orin has no GIC/timer driver yet) — all make the GUI/main-loop code
// below unreachable in those builds.
#[cfg_attr(any(feature = "bootlog", feature = "usbdebug", feature = "baremetal", feature = "tegra"), allow(unreachable_code))]
fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    // BPACE origin. The FIRST instruction of the kernel proper, before any subsystem exists, so the
    // ledger's `t=0` is as close to "the kernel got control" as this function can get. On x86 the
    // rdtsc value read here is the counter since RESET, so it is also — approximately — the cost of
    // firmware + bootloader, the one boot phase this kernel can never instrument from the inside.
    // Approximately: the TSC's zero is the last processor reset, which on a warm boot need not be
    // the moment power was applied. Read the number as an upper bound on pre-kernel time, not as a
    // measurement of firmware. Heap-free and lock-light, so it is safe this early.
    unaos_kernel::bootpace::record("entry"); #[cfg(all(target_arch = "aarch64", feature = "orinfurn"))] tegra_stk_anchor(); // ORIN-STKDEPTH — the FIRST of two SP reads whose difference is a boot-core stack DEPTH in bytes. Anchored on the same statement `bootpace` calls "the FIRST instruction of the kernel proper", which is the earliest stable point on the frame chain `kernel_main -> tegra_early_stop -> tegra_desk_furn`; the second read is beside `tegra_desk_furn`'s `[orinfurn] arm` line. See the ORIN-STKDEPTH block at this file's tail for what the number is and — at length — what it is NOT. ⚠ LINE-NEUTRAL append, and `#[cfg]`-erased on every arch and every build without `orinfurn`, so no panic `Location` moves and the knob-off image is untouched.

    // 0-WC. VPERF-WC HOIST (bootpace.md §11). Retype the framebuffer's identity-map leaves to
    //     Write-Combining BEFORE the console's first paint instead of after it.
    //
    //     `fbcon::init` ends by calling `memory::set_framebuffer_wc` — but it BEGINS with
    //     `fill_screen(BG_DEFAULT)`, a full-surface clear of every visible pixel (2880x1800x4 =
    //     20.7 MB on the bench panel) written one dword at a time. Ordered as it was, that clear
    //     paid the UNCACHEABLE rate the retype exists to escape (~160 MB/s vs ~1.47 GB/s measured,
    //     §10d), and it is the largest term inside `BPACE: heap d=253ms`. Nothing in the retype
    //     depends on fbcon: it needs only the base/length pair, which BootInfo already carries
    //     here, and it already ran at exactly this point in boot — inside a call three lines below.
    //
    //     `set_framebuffer_wc` is self-latching (`FB_WC_DONE`) and no-ops on a zero base/length, so
    //     the call left in place at the end of `fbcon::init` becomes an idempotent second call and
    //     the retype still happens exactly once. The two BPACE stamps (`fb-wc`, `fb-wc-done`) sit
    //     inside the latch and therefore MOVE with the retype — which is how the ledger says on its
    //     own wire which of the two orderings a build has.
    //
    //     Watched side effect: the `:: x86 fb-wc: retyped N leaf(s) ... ::` line is now emitted
    //     before fbcon is ready, so it reaches serial/FTDI but is no longer painted on the panel.
    //     It was never durable on glass anyway — under the old ordering the clear preceded it, and
    //     under this one the clear follows it.
    #[cfg(target_arch = "x86_64")]
    unaos_kernel::arch::memory::set_framebuffer_wc(
        boot_info.framebuffer_addr,
        boot_info.framebuffer_size as u64,
    );

    // 0. Framebuffer log sink FIRST — mirror every serial_println! (and panics) to the screen,
    //    so boot diagnostics are visible on real hardware that has no serial port. No-op if the
    //    firmware gave us no framebuffer. The GUI repaints over it later on a successful boot.
    unaos_kernel::video::fbcon::init(
        boot_info.framebuffer_addr,
        boot_info.framebuffer_size,
        boot_info.framebuffer_info,
    );

    // 0a. WRITER-SEED (x86). `video::WRITER` and fbcon are two handles to the SAME physical
    //     framebuffer, and `WRITER` is the one every panel-geometry consumer reads (`wm`'s
    //     composite path, `cursor`, `screen`, `desktop_uefi::activate`). Until this seam existed, x86
    //     seeded it only far down `kernel_main` (step 3's `framebuffer_addr != 0` block), which is
    //     AFTER the Kepler takeover runs — so `desktop_uefi::activate` found `is_ready() == false` and
    //     declined the console window on metal (`[wc-x] activate DECLINE reason=fb-not-ready`)
    //     while fbcon was fully live on the very same surface. A `WRITER` that lies about the
    //     panel is a general defect, not a compositor one, so this is UNCONDITIONAL — not gated on
    //     `wc`. It takes the identical BootInfo triple fbcon just took, so the two handles cannot
    //     disagree by construction; `FrameBufferInfo.stride` is in PIXELS on both sides (the byte
    //     pitch fbcon witnesses is `stride * bytes_per_pixel`, derived at print time), so there is
    //     no unit conversion to get wrong. Precedent: the aarch64 tegra path already pairs
    //     `fbcon::init` with the same `WRITER.lock().init` on the same triple (JD2, below); the
    //     later step-3 seeding stays exactly as it is and is now an idempotent re-init.
    #[cfg(target_arch = "x86_64")]
    if boot_info.framebuffer_addr != 0 && boot_info.framebuffer_size != 0 {
        unaos_kernel::video::WRITER.lock().init(
            boot_info.framebuffer_addr as usize,
            boot_info.framebuffer_size,
            boot_info.framebuffer_info,
        );
        let i = boot_info.framebuffer_info;
        serial_println!(
            ":: video: WRITER seeded base={:08X} len={} panel={}x{} stride={}px pitch={}B bpp={} ::",
            boot_info.framebuffer_addr,
            boot_info.framebuffer_size,
            i.width,
            i.height,
            i.stride,
            i.stride * i.bytes_per_pixel,
            i.bytes_per_pixel,
        );
    }

    // 0a2. EDID-CARRY. The UEFI bootloader reads the panel's EDID (ACTIVE protocol, then
    //      DISCOVERED) to pick the display mode, and until now kept only the native width and
    //      height out of it — the pixel clock and the h/v blanking and sync numbers, which is what
    //      programming a display pipe actually needs, were read and dropped inside the bootloader.
    //      `BootInfo::edid_block` now carries the 128-byte base block, and this is where it is
    //      published (`video::edid_block()`) and witnessed.
    //
    //      UNCONDITIONAL, on both arches and every build, for the same reason the WRITER seed above
    //      is: a panel descriptor that exists only under one feature flag cannot be read from a
    //      metal capture of the build that is actually flown. It must also be able to say NO — on
    //      QEMU and on the Pi's bare-metal path there is no EDID protocol at all, and the line
    //      prints `present=0` there. Runs before `arch::memory::init` consumes `boot_info`.
    unaos_kernel::video::init_edid(
        &boot_info.edid_block,
        boot_info.edid_block_valid,
        boot_info.edid_total_len,
    );

    // 0b. Jetson Orin Nano (tegra): install the kernel's own MMU (JM3), bring up the GIC + timer on the
    //     boot core (JM4), then drop EL2 -> EL1 and run the scheduler + CAPSTONE at EL1 (JM6). The
    //     UEFI-handoff tables map RAM but NOT the Tegra peripheral MMIO (JM2 R4: the kernel faulted on its
    //     first UARTC read), so `tegra_early_stop` first calls `mmu_tegra::init` to map RAM Normal-WB + the
    //     Tegra device window Device-nGnRE — which is what lets the serial path drive UARTC — then brings
    //     up the Tegra234 GIC-600 + the generic timer (their GIC bases sit in that mapped device window)
    //     and proves the timer PPI at EL2, and finally (JM6) drops the boot core EL2 -> EL1 and runs the
    //     M4 CAPSTONE cooperatively at EL1. It diverges before the rest of `kernel_main` (heap/GUI and
    //     Orin userspace/SMP — later arcs), so everything below is unreachable on tegra (covered by the
    //     fn's `allow(unreachable_code)`). `tegra` is off in every QEMU build, so this is inert there and
    //     the regression logs are byte-identical.
    #[cfg(all(feature = "tegra", target_arch = "aarch64"))]
    tegra_early_stop(boot_info);

    // Gated on intel-ivb + x86_64 too, not just unaos_ivb: drivers::gpu does not exist on
    // aarch64 (this exact gate was dropped once before and broke the aarch64 check — keep it).
    #[cfg(all(feature = "unaos_ivb", feature = "intel-ivb", target_arch = "x86_64"))]
    if boot_info.igpu_trace_valid {
        unaos_kernel::drivers::gpu::igpu::set_boot_traces(
            boot_info.igpu_trace_0,
            boot_info.igpu_trace_1, 
            boot_info.igpu_trace_2,
            boot_info.gmux_trace_0
        );
    }

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

    // SPLASH-1 (x86 GUI builds only): paint the ray-traced prism boot splash NOW — before the
    // slow bring-up (ACPI, SMP, xHCI enumeration) — replacing the blank pre-GUI panel while boot
    // works. Allocation-free (pre-heap by design); one background fill + the traced rays, so it
    // does not slow boot measurably. fbcon's QUIET-PANEL milestone lines paint over it (they stay
    // the boot witness surface); the GUI's background paint replaces it at handoff. Never on
    // usbdebug/witness/bootlog builds — their panels carry the boot log, and the test/bench
    // batteries stay byte-identical.
    #[cfg(all(
        target_arch = "x86_64",
        not(any(feature = "usbdebug", feature = "bootlog", feature = "witness"))
    ))]
    if framebuffer_addr != 0 {
        unaos_kernel::splash::boot_splash(framebuffer_addr as usize, framebuffer_size, info);
    }

    // EDID/mode-selection diagnostics (read before memory::init consumes boot_info); only the
    // bootlog build uses them, so gate the extraction to avoid unused-field warnings elsewhere.
    #[cfg(feature = "bootlog")]
    let (edid_native_w, edid_native_h, edid_source, mode_action) = (
        boot_info.edid_native_width,
        boot_info.edid_native_height,
        boot_info.edid_source,
        boot_info.mode_action,
    );

    // INSTALL-SELF: publish the boot volume's FAT serial into the installer's boot-device guard BEFORE
    // memory::init consumes boot_info. This is the whole handoff — the bootloader read the serial off
    // the ESP it loaded this kernel from, and from here on the installer can recognize (and refuse to
    // erase) the device the system is running from. Gated exactly like `crate::install` itself, so a
    // build without an installer is byte-identical to baseline. 0 (aarch64, or an unidentifiable boot
    // volume) disarms the guard with a witness line; it never blocks a boot.
    #[cfg(any(feature = "installdemo", feature = "install_target", feature = "piinstall"))]
    unaos_kernel::install::selfguard::set_boot_volume_serial(boot_info.boot_volume_serial);

    // FRGUARD (GR21): the SAME field, published a second time — into the block layer's Default-write
    // substitution guard. Deliberately not sharing INSTALL-SELF's copy above: that one is gated on the
    // installer features, which no bench or boot build carries, and that is exactly why its
    // `:: install: boot volume serial …` witness appears ZERO times across the 30-boot capture at
    // capture/rmbp-gr16-s73. A guard whose own input is invisible on the wire can be neither trusted
    // nor falsified, so this arm carries its own publication and its own witness. Gated to the builds
    // that compile the guard, so every other target is untouched.
    #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
    unaos_kernel::drivers::block::set_boot_volume_serial(boot_info.boot_volume_serial);

    // JC3 (virt/UEFI, GICv3 only): capture the firmware RAM-GiB map from boot_info BEFORE memory::init
    // consumes it (it takes the `&'static mut`), so the EL2->EL1 drop below can build the boot core's EL1
    // identity map. Runtime-gated on `is_v3()` so the GICv2 virt run computes nothing beyond the cheap GIC
    // check; compiled only on the `virt` build (baremetal has boot.rs; tegra keeps its EL2 regime).
    #[cfg(all(target_arch = "aarch64", not(feature = "pi"), not(feature = "tegra")))]
    let jc3_ram_gib_mask = if unaos_kernel::arch::gic::is_v3() {
        unaos_kernel::arch::boot_virt::ram_gib_mask(boot_info)
    } else {
        0
    };

    // 4. Global Heap Allocation (Phase 3 Memory Translation)
    unaos_kernel::arch::memory::init(boot_info);
    serial_println!(":: KERNEL HEAP ALLOCATED ::");
    unaos_kernel::bootpace::record("heap");

    // VPERF M3 EARLY-ATTACH (bench QoL, Peter's word 2026-07-16): usbdebug builds attach the
    // fbcon cached-RAM shadow the moment the heap exists, so the ENTIRE boot log scrolls in
    // cached RAM instead of read-modify-write uncached VRAM (the pre-shadow scroll dominated
    // sitting wall-clock on the rMBP). GUI builds still never attach — the Screen back buffer
    // owns the heap budget there — and the original post-heap attach site below remains as an
    // idempotent no-op.
    #[cfg(all(target_arch = "x86_64", feature = "usbdebug"))]
    unaos_kernel::video::fbcon::attach_shadow();

    // 4a. aarch64 SMP (bare-metal Pi 4): release the 3 parked Cortex-A72 secondary cores from the
    // firmware spin-table. Each brings up its own MMU + exception vectors and (Milestone 1) reports
    // in over serial, then idles. The BSP continues below as the hardware-service core.
    #[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
    unaos_kernel::arch::smp::start_secondaries();

    // 4a'. aarch64 SMP on the QEMU `virt` GICv3 path (JC2): bring the 3 PSCI-parked secondaries online
    // via PSCI CPU_ON + per-core GICv3 bring-up, proven by cross-core SGI. Compile-gated off every Pi
    // image (baremetal implies pi), and runtime-gated on GICv3 detection so the plain GICv2 virt run
    // stays single-core (byte-identical to baseline). The APs park in WFI; there is no scheduler on the
    // virt path (it is baremetal-gated + EL1-coupled — see the JC2 brief). Uses only static state (no
    // heap). dtb_addr/dtb_size (captured above) let it confirm the PSCI conduit from the live DTB.
    //
    // Additionally gated OFF for the `tegra` build. An esp-jetson kernel is a not-baremetal build
    // whose `ID_AA64PFR0_EL1.GIC` reads v3 on Orin silicon, so without this gate the kick-off would
    // run and walk the hardcoded QEMU-`virt` `GICR_BASE` (0x080A_0000) — unmapped MMIO on Tegra234.
    // (The tegra early stop above already diverges before reaching here, but this compile-time gate
    // is the load-bearing guarantee: even if that stop later moves, the tegra image never touches the
    // virt GICR. JM3 brings up the real Orin redistributor.)
    #[cfg(all(target_arch = "aarch64", not(feature = "pi"), not(feature = "tegra")))]
    if unaos_kernel::arch::gic::is_v3() {
        unaos_kernel::arch::smp_virt::start_secondaries(dtb_addr, dtb_size);

        // ORIN-NET-1 (QEMU graceful-skip WITNESS, `UNAOS_PCIEPROBE=1`): run the SAME read-only PCIe
        // census on the virt GICv3 path so the arc has a runnable QEMU gate. The QEMU virt DTB has a
        // generic `pci-host-ecam-generic` node but NO Tegra234 root complex, so the census dumps the
        // generic controller and reports "no Tegra234 PCIe RC (graceful)" for the config gate — then
        // CAPSTONE completes below. Metal RAM map is the JC3 mask. Compiled out knob-off, so the
        // default virt regression is byte-identical to baseline. See arch_arm64.md §ORIN-NET-1.
        #[cfg(feature = "pcieprobe")]
        unaos_kernel::arch::pcie_probe::census(&unaos_kernel::arch::pcie_probe::PcieCtx {
            dtb_addr,
            dtb_size,
            ram_gib_mask: jc3_ram_gib_mask,
        });

        // ORIN-NET-2 (QEMU graceful-skip WITNESS, `UNAOS_PCIE2=1`): same read-only recon on the virt
        // GICv3 path. The GICv3 handoff leaves dtb_addr=0, so census2 hits its "no DTB handed off —
        // recon SKIPPED (graceful)" line and returns before any MMIO; CAPSTONE completes below.
        // Compiled out knob-off. See arch_arm64.md §ORIN-NET-2.
        #[cfg(feature = "pcie2")]
        unaos_kernel::arch::pcie_probe::census2(&unaos_kernel::arch::pcie_probe::PcieCtx {
            dtb_addr,
            dtb_size,
            ram_gib_mask: jc3_ram_gib_mask,
        });

        // ORIN-NET-3 (QEMU PS-widen mapping WITNESS, `UNAOS_PCIE3=1`): the metal TCR widen is only
        // programmed on the tegra boot, but the DECISION it changes — `map_mmio_window`'s reach
        // ceiling — is exercised here. The witness inverts NET-2's regression: the controller-0 ECAM
        // (~184 GiB) that NET-2 refused must now be REACHABLE, and refusal must persist above the
        // reachable range. Prints `ORIN-NET-3 PS-widen witness: PASS`. Compiled out knob-off. See
        // arch_arm64.md §ORIN-NET-3.
        #[cfg(feature = "pcie3")]
        unaos_kernel::arch::pcie_probe::ps_widen_witness();

        // ORIN-NET-4 (QEMU witness, `UNAOS_NET4=1` without `UNAOS_TEGRA=1`): QEMU virt models no
        // Tegra234 RC, so the RTL8168 driver has no device to claim. The `not(tegra)` build of
        // `net4_bringup` prints one honest line recording that the driver is compiled-present but its
        // bring-up is metal-only, and returns before any MMIO — keeping the GICv3 regression run
        // unperturbed. Compiled out knob-off. See arch_arm64.md §ORIN-NET-4.
        #[cfg(all(feature = "net4", not(feature = "tegra")))]
        unaos_kernel::arch::rtl8168_tegra::net4_bringup(dtb_addr, dtb_size, jc3_ram_gib_mask);

        // ORIN-SDMMC-1 (QEMU witness, `UNAOS_SDMMC=1` without `UNAOS_TEGRA=1`): QEMU models no Tegra234
        // SDMMC controller, so the microSD recon has nothing to census. The `not(tegra)` build of
        // `sdmmc_census` prints one honest compiled-present line and returns before any MMIO — keeping the
        // GICv3 regression run unperturbed. Compiled out knob-off. See arch_arm64.md §ORIN-SDMMC.
        #[cfg(all(feature = "sdmmc", not(feature = "tegra")))]
        unaos_kernel::arch::sdmmc_tegra::sdmmc_census(dtb_addr, dtb_size, jc3_ram_gib_mask);

        // AARCH64-VNET (QEMU witness, `UNAOS_VNET=1`): drive a `virtio-net-device` on the `virt`
        // machine's virtio-mmio bus end-to-end — the QEMU-testable proof of the aarch64 smoltcp seam
        // NET-4 built (identical ring → phy::Device → Interface → ICMP-echo shape, with REAL packets
        // over slirp). Runs HERE, at EL2 before the JC3 drop, where the heap is up and the virtio-mmio
        // window (0x0a00_0000, low-1-GiB Device map) is reachable; the bounded ICMP-ping witness
        // completes synchronously and emits a self-checking PASS/FAIL line. Needs `UNAOS_VNET=1`'s
        // `-netdev user -device virtio-net-device` QEMU args (added in arroyo behind the same knob).
        // Compiled out knob-off => the GICv3 regression run is byte-identical. See arch_arm64.md
        // §AARCH64-VNET.
        #[cfg(feature = "vnet")]
        unaos_kernel::arch::virtio_net::vnet_bringup();

        // JC3: with the JC2 SMP proof complete (the secondaries are parked at EL2 in their WFI loop), drop
        // the BOOT CORE EL2 -> EL1 and run the scheduler + full M4 CAPSTONE there — the QEMU-testable proof
        // that the scheduler and all six sync primitives run at EL1 on the GICv3 `virt` path. Sequenced
        // AFTER the SMP proof so the two never fight (the APs are parked before the BSP changes its EL);
        // only the BSP drops (the APs stay at EL2). This DIVERGES — the boot core becomes the CAPSTONE core
        // and never returns — so the GICv3 virt run ENDS here (no GUI/USB below); the GICv2 (!is_v3) run is
        // untouched and falls through to the normal boot path.
        //
        // Detach fbcon first: after the drop the boot core's EL1 map covers only RAM + the low peripheral
        // window (PL011/GIC), and the firmware framebuffer may live outside both (e.g. a high PCI BAR), so
        // a serial_println! that mirrored to it would fault at EL1. Serial itself (PL011, in the mapped
        // Device window) stays live, so the CAPSTONE log is captured regardless.
        unaos_kernel::video::fbcon::detach();
        serial_println!(
            ":: JC3: SMP proof done; dropping the virt boot core EL2 -> EL1 for the scheduler + CAPSTONE ::"
        );
        unsafe { unaos_kernel::arch::boot_virt::drop_to_el1(jc3_ram_gib_mask) };
        // Now at EL1 with the MMU live and DAIF masked. Re-seed this core's per-CPU block (now TPIDR_EL1)
        // and install the EL1 exception vectors (VBAR_EL1), then run CAPSTONE cooperatively (never returns).
        unaos_kernel::arch::percpu::init(0);
        unaos_kernel::arch::exceptions::install();
        unaos_kernel::arch::sched::run_capstone_boot_core(0);
    }

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

        // INROUTE: the router selftest runs HERE — before `start_aps`, and therefore before the first user
        // slot in this boot exists. Its correctness depends on OWNING the global input focus and
        // `pal::EVENT_QUEUE` for the length of the test, and this is the only point in the boot where that
        // ownership is structural rather than hoped for. See `input_router_selftest` for the race this
        // placement closes.
        input_router_selftest();
        // SERIAL-FOCUS: the source-split fixture, immediately after the router selftest and for the
        // same structural reason — it fakes GUI focus onto an ASID and must be the only owner of the
        // global focus while it measures. It runs SECOND because `input_router_selftest` asserts a
        // `GUI_SENT` delta of its own, and this one must not perturb that window.
        #[cfg(feature = "witness")]
        serial_focus_selftest();

        unaos_kernel::arch::sched::start_aps(&online);

        // U11-M2b (U11-REAP ORDERING FIX): the deferred-free REAPER is spawned HERE — the first point in
        // the boot where a scheduled core exists — and NOT down in the panel-service block it used to live
        // in. Its own doc comment states the invariant it depends on ("Spawned at BOOT ... so it already
        // exists before any orphan is queued"), and the old site did not honour it: the panel block runs
        // AFTER `pi_rast_demo_maybe()`, while the whole EL0 fixture cascade — U11-reap included — is
        // already running on the APs. A teardown-orphaned chain queued by that fixture therefore waited on
        // a service that did not exist yet, and U11-reap's bounded CHECKPOINT-3 poll passed only when the
        // raster demo happened to finish inside it: 4.5 s of demo (PASS, by half a second) vs 9.5 s with
        // `UNAOS_PIDESK=1 UNAOS_PIRAST=1` (deterministic FAIL, the chain freed 300+ lines after the
        // verdict). Availability of a kernel service must not be a function of demo/panel composition.
        // Placement is unchanged (`spawn_auto`, load-balanced, SCHED-3b); only ONLINE_MASK's AP set is
        // registered at this point, so the reaper can never be placed on the not-yet-scheduling BSP. It
        // blocks on `REAPER_SEM` until something is queued, so an early spawn costs an idle task and no
        // duty cycle. Unconditional (no framebuffer gate): a panel-less boot has teardown orphans too.
        unaos_kernel::arch::sched::spawn_auto(
            "orphan-reaper",
            unaos_kernel::arch::syscall::orphan_reaper,
            0,
        );

        // M6g Part B: probe the microSD (EMMC2 first, legacy SDHCI fallback) and register it as the block
        // backend. Synchronous, on the BSP, BEFORE the M6b demo — single-threaded mailbox use (the boot
        // framebuffer call is long done) and deterministic serial placement: its two lines land early,
        // before the demo lines. The M6g loader (spawned below) later reads the FAT volume off this card.
        #[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
        unaos_kernel::drivers::emmc2::probe();

        // ONECARD-PI: name the ONE medium this system runs from, immediately after the backend that
        // serves it is registered — the card's geometry, its FAT program volume, its native volume's
        // extent, and the USB census. Unconditional (not behind `witness`): the claim "UnaOS boots
        // and runs from a single card" is a property of a DEFAULT boot, so a default boot is exactly
        // the capture that has to carry its evidence. One line, reads only. See the function doc for
        // why the audit produced a witness rather than a fix.
        #[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
        unaos_kernel::drivers::emmc2::onecard_witness();

        // PI-SHELL-LS (witness battery): prove the Pi shell's `ls` lists the native unafs volume — the
        // same store PI-NET-15 serves at `/fs/`. The verb is panel-only on the bench, so this exercises
        // the exact `pi_ls_collect` listing headlessly and emits the `:: ls1: /: ... ::` witness a
        // `UNAOS_PI=1 ./arroyo kernel8-test` capture can verify. Quiet default boots compile none of it.
        #[cfg(all(target_arch = "aarch64", feature = "baremetal", feature = "witness"))]
        unaos_kernel::shell::pi_ls_witness();

        // MIDDEN-M1 (witness battery): the shell's interpreter is now `unaos/libs/sys/midden_core`,
        // shared with the Ring 3 `midden` handler. The live `:: [midden] ... ::` line needs a
        // keystroke and the headless gates type nothing, so this drives the SAME `plan()` the
        // prompt drives — core-answer, host-routing, `.elf`-elided resolution, and verb-beats-
        // program precedence — over a synthetic volume, in the uniform `:: TSTE: ... ::` shape.
        // Arch-neutral by construction (the core carries no `cfg`), so the identical four lines
        // land on the x86 battery below.
        #[cfg(all(target_arch = "aarch64", feature = "baremetal", feature = "witness"))]
        unaos_kernel::shell::midden_witness();

        // INSTALL-PI: the installer engine's first LIVE end-to-end execution — GPT → FAT32 → payload copy
        // → sha extent-verify onto the emmc2 microSD just censused above. Three-gate escalation (census /
        // scratch-ladder / destructive-confirm), all `piinstall*`-gated, so a default build compiles NONE
        // of this and this call site vanishes. In QEMU `raspi4b` the SD slot carries a DEDICATED BLANK
        // scratch image (the `./arroyo kernel8-install` witness), never the battery fixture.
        #[cfg(feature = "piinstall")]
        unaos_kernel::install::pi::run();

        // PIUSB-33: the SPLIT USB bring-up. The RC + xHCI HARDWARE bring-up (brcmstb RC reset/PERST/CNR
        // settle → controller halted-but-decoding + ports powered) already ran EARLY, inside
        // `piusb::bringup` in build_boot_info — the P38-proven single-threaded pre-V3D/pre-GENET/pre-panel
        // context. Metal P39/P40/P41 proved the first RC APB read HARD-STALLS on ANY core in the deferred
        // post-panel context; P43 exonerated firmware power/clock; so the RC read must live in the early
        // context. `piusb::bringup_task(0)` now runs the DEFERRED half ONLY: the heap-backed DMA-side walk
        // (`enumerate` — rings/interrupter, RS=1, port/HID/storage enumeration), which touches the xHCI BAR
        // MMIO (not the stalling RC APB) and needs the heap. It runs on the BSP AFTER the GUI/input/render
        // tasks are spawned (see the call just before the BSP idles), so the panel is live while it walks.
        // HERE we handle only the no-AP / serial-only fallback that never spawns those GUI tasks: enumerate
        // synchronously before the shared BSP loop below polls the controller. QEMU raspi4b census-skips.
        #[cfg(feature = "piusb")]
        if online.is_empty() {
            unaos_kernel::arch::piusb::bringup_task(0);
        }

        // PI-GENET: the BCM2711 on-board Gigabit Ethernet (GENET v5) + smoltcp bind — the Pi's FIRST
        // network path. DTB-resolves the register base, poison-honest probes SYS_REV_CTRL to classify
        // whether this build models GENET (QEMU raspi4b MAY), brings up UMAC + PHY + TDMA/RDMA rings,
        // and binds a smoltcp Device + DHCP/ping. Post-heap on the BSP (rings need the heap). Graceful
        // skip on an absent decode. Default OFF => this call + the module vanish (byte-identical).
        #[cfg(feature = "genet")]
        unaos_kernel::arch::genet::genet_bringup(dtb_addr, dtb_size);

        // M6b: EL0 fault isolation + per-page user permissions. Four EL0 programs on one AP (never
        // the unscheduled BSP): hello (must still work — the code page is user-RX), then three that
        // each provoke a specific fault the kernel must answer by KILLING THE TASK, not halting —
        // a write to kernel RAM, a write to the now-read-only code page, a jump into the UXN stack
        // page. A verdict task on a DIFFERENT core (a wedged demo core must still produce a FAIL
        // line — the guarantee needs >= 2 online APs; the spawn log below discloses the cores, so
        // a degraded single-AP boot is visible) demands the exact outcome split. Flow: copy the blob (code page still RW) -> warm
        // the demo core's TLB with the OLD mapping (so a broken broadcast TLBI is deterministically
        // visible on metal instead of silently passing) -> protect (the kernel's first live
        // page-table update + TLBI) -> spawn. All synchronous exceptions: fully QEMU-verifiable.
        //
        // DEFAULT-QUIET: the whole M6b/M6e/M6d/M6f/M6g + U4/U5/U6/U6b/U7 EL0 fixture flow (U7 in turn
        // cascades U9/U10/U10c/U10d/U11/U11-defer/U11-reap + the K*/F2/F3/BANDY/unafs selftests) re-proves
        // long-metal-confirmed facts on every kernel8 boot — behind the `witness` battery knob. The APs
        // (start_aps above) + the boot-honesty CAPSTONE workload + emmc2 probe stay unconditional, so a
        // quiet default boot reaches the shell with CAPSTONE + device lines only.
        #[cfg(feature = "witness")]
        if let Some(&cpu) = online.first() {
            let demo = unaos_kernel::arch::syscall::setup();
            unaos_kernel::arch::sched::spawn(
                "tlb-warm",
                unaos_kernel::arch::syscall::tlb_warm,
                0,
                cpu,
            );
            // Bounded BSP wait for the warm-up (~500 ms; QEMU: microseconds). Proceed on timeout —
            // the demo still runs; only the deterministic TLBI detection is degraded.
            let t0 = unaos_kernel::arch::timer::cntpct();
            let budget = unaos_kernel::arch::timer::cntfrq() / 2;
            while !unaos_kernel::arch::syscall::TLB_WARMED
                .load(core::sync::atomic::Ordering::Acquire)
                && unaos_kernel::arch::timer::cntpct().wrapping_sub(t0) < budget
            {
                core::hint::spin_loop();
            }
            unaos_kernel::arch::syscall::protect();
            unaos_kernel::arch::sched::spawn_user("el0-hello", demo.hello, demo.sp, cpu);
            unaos_kernel::arch::sched::spawn_user("el0-wild-write", demo.wild_write, demo.sp, cpu);
            unaos_kernel::arch::sched::spawn_user("el0-code-write", demo.code_write, demo.sp, cpu);
            unaos_kernel::arch::sched::spawn_user("el0-stack-exec", demo.stack_exec, demo.sp, cpu);
            let vcpu = online.get(1).copied().unwrap_or(cpu);
            unaos_kernel::arch::sched::spawn(
                "m6b-verdict",
                unaos_kernel::arch::syscall::verdict,
                0,
                vcpu,
            );
            serial_println!(
                ":: M6b: EL0 fault-isolation demo — 4 programs on core {}, verdict on core {} ::",
                cpu,
                vcpu
            );

            // M6e: preemptible EL0. `spawn_user` now starts the task with IRQ UNMASKED (SPSR 0x240)
            // and `__vec_irq` banks SP_EL0, so the generic timer can preempt a running EL0 task. The
            // spinner is a long, register-only, syscall-free user loop on the demo core; on metal the
            // timer preempts it mid-loop — it interleaves with the co-located capstone/kernel tasks
            // and `aarch64_irq_handler` counts each EL0 IRQ; the m6e-verdict on the sibling core
            // reports the count. Metal-only: QEMU raspi4b delivers no Group-1 timer IRQ, so the
            // spinner runs its bounded loop uninterrupted there (count stays 0), never hanging the
            // regression. The verdict shares `vcpu` with the M6b verdict / capstone workers and polls
            // via `yield_now`.
            serial_println!(":: M6e: EL0 preemptible (SP_EL0 banked; spawn_user I-unmasked) ::");
            unaos_kernel::arch::sched::spawn_user("el0-spin", demo.spin, demo.sp, cpu);
            unaos_kernel::arch::sched::spawn(
                "m6e-verdict",
                unaos_kernel::arch::syscall::m6e_verdict,
                0,
                vcpu,
            );

            // M6d: per-task address spaces (ASIDs) + per-task user stacks. Allocate four PRIVATE
            // address-space slots (own translation-table branch + own 16 KiB backing at the SAME VAs,
            // ASID-tagged) and drop four user tasks onto them via `spawn_user_slot`: two read distinct
            // slot-private sentinels at the identical VA (same-VA isolation), one WRITES and reads back
            // its own stack (the capability this arc unlocks — impossible on the shared window), one
            // spins-then-reads through SP (SP_EL0 fidelity across preemption on metal). Unlike M6b/M6e,
            // isolation is visible WITHOUT interrupts, so the same-VA/stack lines are fully QEMU-provable;
            // a deterministic kernel-side nG probe (folded into the same-VA PASS) is the metal detector.
            // The verdict shares `vcpu` with the M6b/M6e verdicts.
            if let Some(m6d) = unaos_kernel::arch::syscall::m6d_setup() {
                unaos_kernel::arch::sched::spawn_user_slot(
                    "el0-samevaA", m6d.same_va, m6d.sp, m6d.ttbr0_a, cpu,
                );
                unaos_kernel::arch::sched::spawn_user_slot(
                    "el0-samevaB", m6d.same_va, m6d.sp, m6d.ttbr0_b, cpu,
                );
                unaos_kernel::arch::sched::spawn_user_slot(
                    "el0-stackwrite", m6d.stack_write, m6d.sp, m6d.ttbr0_stack, cpu,
                );
                unaos_kernel::arch::sched::spawn_user_slot(
                    "el0-spsentinel", m6d.sp_sentinel, m6d.sp, m6d.ttbr0_sp, cpu,
                );
                unaos_kernel::arch::sched::spawn(
                    "m6d-verdict",
                    unaos_kernel::arch::syscall::m6d_verdict,
                    0,
                    vcpu,
                );
            } else {
                serial_println!(":: M6d: slot allocation failed — per-task address-space demo SKIPPED ::");
            }

            // M6f: validated user pointers (copy_from_user/copy_to_user) + a wider syscall surface
            // (YIELD/SLEEP_MS/GETPID/GETINFO). Four user fixtures on PRIVATE slots (the getinfo fixture
            // writes its stack via copy_to_user, forbidden on the shared window): a well-behaved getinfo
            // round-trip (copy_to_user then read-back matches SYS_GETPID), a hostile-pointer fixture whose
            // four bad pointers must each ERROR-RETURN -EFAULT (never a kill), and a yield/sleep pair that
            // cooperatively interleaves on `cpu` (the two must share a core to interleave under QEMU, which
            // has no preemption). Verdict on the sibling `vcpu`. M6d holds 4 slots, M6f takes the other 4 —
            // exactly the 8-slot cap. Essentially QEMU-provable (validation + copies under ASIDs the metal
            // already proved); the per-task preempt counter goes > 0 on metal (rides along with M6g).
            if let Some(m6f) = unaos_kernel::arch::syscall::m6f_setup() {
                unaos_kernel::arch::sched::spawn_user_slot(
                    "el0-getinfo", m6f.getinfo, m6f.sp, m6f.ttbr0_getinfo, cpu,
                );
                unaos_kernel::arch::sched::spawn_user_slot(
                    "el0-hostile", m6f.hostile, m6f.sp, m6f.ttbr0_hostile, cpu,
                );
                unaos_kernel::arch::sched::spawn_user_slot(
                    "el0-yield", m6f.yield_prog, m6f.sp, m6f.ttbr0_yield, cpu,
                );
                unaos_kernel::arch::sched::spawn_user_slot(
                    "el0-sleep", m6f.sleep_prog, m6f.sp, m6f.ttbr0_sleep, cpu,
                );
                unaos_kernel::arch::sched::spawn(
                    "m6f-verdict",
                    unaos_kernel::arch::syscall::m6f_verdict,
                    0,
                    vcpu,
                );
            } else {
                serial_println!(":: M6f: slot allocation failed — validated-user-pointer demo SKIPPED ::");
            }

            // M6g: load a program FROM STORAGE and run it at EL0 — the Pi twin of x86 U2. A kernel task
            // on `vcpu` (a scheduled sibling of the demo core): it waits for the M6f verdict, mounts the
            // FAT volume off the SD card the Part-B probe registered, reads HELLO.BIN (the M6c blob bytes,
            // carried onto the boot media), copies it into a fresh M6d slot, and drops it to EL0. The
            // loaded bytes are untrusted — contained by EL0 + per-page perms + the M6b fault-kill net. It
            // doubles as its own verdict. No-op (one skip line) when no SD block device was registered.
            unaos_kernel::arch::sched::spawn(
                "m6g-loader",
                unaos_kernel::arch::syscall::m6g_loader,
                0,
                vcpu,
            );

            // U4: the process model + per-process handle table — sys_spawn (returns a HANDLE) + sys_wait
            // (reaps by handle). A gated kernel task on `vcpu` (the m6g-loader idiom), with the demo core
            // `cpu` passed as its arg: it waits for the M6g loader to finish (so its lines print first AND the
            // M6d/M6f/M6g slots have freed), builds the PARENT's and ORPHAN's slots, and spawns both on `cpu`.
            // The parent's two sys_spawns load HELLO.BIN off the SD card into fresh slots and run them in user mode
            // as CHILDREN co-located on `cpu`, each installed as a handle in the parent's table; sys_wait
            // reaps each by handle (a scheduler wake — QEMU-testable). The orphan's sys_wait on an unheld
            // handle returns -ECHILD (structural ownership). The launcher folds the verdict. (Deferred to
            // run-time because M6d+M6f hold all 8 slots at BSP-wiring time; they free as their fixtures exit
            // — hence the M6g gate.)
            unaos_kernel::arch::sched::spawn(
                "u4-launch",
                unaos_kernel::arch::syscall::u4_launcher,
                cpu,
                vcpu,
            );

            // U5: handles as CAPABILITIES — the enforcement layer on top of U4's handle STRUCTURE. A gated
            // kernel task on `vcpu` (the u4-launch idiom), demo core `cpu` as its arg: it waits for the U4
            // verdict (U4_LAUNCH_DONE), builds + pre-endows a single fixture slot, and runs `el0-u5cap` on
            // `cpu`. That fixture proves, against its own per-process table, the four user-observable
            // capability semantics — a console cap writes; a write-LESS cap gets -EACCES; a grant cannot
            // exceed the granter's rights (attenuation) while a subset grant works; a revoked handle is
            // -EACCES — and the launcher then proves the fifth kernel-side: the fixture's handle row is
            // cleared on slot teardown (no stale capability outlives its ASID). Fully QEMU-verifiable (pure
            // syscall logic; no disk). Gated after U4 for the same reason U4 gates after M6g — the 8 slots
            // free as the prior fixtures exit.
            unaos_kernel::arch::sched::spawn(
                "u5-launch",
                unaos_kernel::arch::syscall::u5_launcher,
                cpu,
                vcpu,
            );

            // U6: the general OBJECT TABLE — the (kind, target, rights) descriptor + first-free allocation for
            // ALL kinds, killing U5's fixed CONSOLE_FD pin. A gated kernel task on `vcpu` (the u5-launch idiom),
            // demo core `cpu` as its arg: it waits for the U5 verdict (U5_LAUNCH_DONE), builds a fixture slot,
            // runs the kernel-side object-table checks (File/Socket kinds resolve; the reserved-index allocator
            // survives the exact console-vs-child interleaving U5 couldn't), then runs `el0-u6spawn` on `cpu` —
            // the printing spawner U5 couldn't serve: it prints, spawns 2 children (distinct auto-allocated
            // handles, off the reserved console index), prints AGAIN (the console cap survived the spawns), and
            // reaps both by handle. Gated after U5 for the same reason U5 gates after U4 — the 8 slots free as
            // the prior fixtures exit. Fully QEMU-verifiable (its children load off the SD, like U4).
            unaos_kernel::arch::sched::spawn(
                "u6-launch",
                unaos_kernel::arch::syscall::u6_launcher,
                cpu,
                vcpu,
            );

            // U6b: real File handles — SYS_OPEN/SYS_READ routed through the object table, making U6a's `File`
            // scaffold real (the first resource syscall on a non-Console object; the precursor to UnaFS grants).
            // A gated kernel task on `vcpu` (the u6-launch idiom), demo core `cpu` as its arg: it waits for the
            // U6 verdict (U6_LAUNCH_DONE), builds a fixture slot, pre-endows it (a File handle WITHOUT CAP_READ
            // + a Socket handle WITH CAP_READ) and plants the expected on-disk prefix, then runs `el0-u6bfile`
            // on `cpu`. That fixture opens HELLO.BIN, reads it through the returned File capability and verifies
            // the bytes, then proves the SYS_READ CHECK denies both a no-CAP_READ File (rights arm) and a
            // non-File Socket (kind arm) with -EACCES. Gated after U6 for the same reason U6 gates after U5 —
            // the 8 slots free as the prior fixtures exit. Fully QEMU-verifiable (reads the SD, like U4/U6).
            unaos_kernel::arch::sched::spawn(
                "u6b-launch",
                unaos_kernel::arch::syscall::u6b_launcher,
                cpu,
                vcpu,
            );

            // U7: cross-process capability transfer — the first CROSS-process op on the object table. A
            // gated kernel task on `vcpu` (the u6b-launch idiom), demo core `cpu` as its arg: it waits for
            // the U6b verdict (U6B_LAUNCH_DONE), builds TWO fixture slots, and orchestrates the delegation
            // script — the parent SYS_XFERs an attenuated Console cap into the child's per-ASID inbox (an
            // over-rights transfer is refused), the child SYS_RECVs it into its OWN handle row and PRINTS
            // through it, the parent then revokes the transfer (sender-owned record) and the child's next
            // use is -EACCES. The launcher also proves the single-writer invariant kernel-side (the child's
            // row is byte-clear while the deposit sits in its inbox) and that teardown leaves no descriptor,
            // inbox slot, or transfer record behind. Fully QEMU-verifiable (cooperative SYS_YIELD polling).
            // U7STK (PARITY §6.1b) — this ONE task gets a RIGHT-SIZED kernel stack, not the blanket
            // 16 KiB `sched::TASK_STACK_SIZE`, and the number is measured rather than chosen:
            //
            //   * QEMU raspi4b, `[u7stk]` high-water, AFTER the `#[inline(never)]` fix that took
            //     `u7_launcher`'s own frame from 5696 B to 16 B:  peak hw = 12296 B (75% of 16 KiB).
            //     BEFORE that fix the same probe read hw = 16384 B, headroom = 0 — the whole stack
            //     touched, surviving by exactly zero bytes.
            //   * Static worst case over the whole call graph reachable from `u7_launcher`:
            //     26880 B before the fix, 14240 B after — i.e. 16 KiB was impossible before, and is
            //     a 2144-byte squeeze after.
            //   * METAL TAKES A PATH QEMU CANNOT. The Pi's USB-storage route adds
            //     `xhci::bot_transfer -> bot_rescue_escalate -> run_bot_stage -> ... ->
            //     fbcon::__print -> wm::composite`, a subtree measuring 5520 B on its own; QEMU
            //     raspi4b models no xHCI and never takes it. 12296 + 5520 = 17816 > 16384, so 16 KiB
            //     is NOT safe on hardware even with the frame fix — which is exactly the trap the
            //     original defect set (three arcs gated green on the path metal does not take).
            //
            // 32 KiB is therefore the smallest power-of-two that clears the measured metal worst case
            // with margin: 1.84x the static worst case, 2.66x the QEMU peak. The cost is 16 KiB of
            // heap for ONE task; raising `TASK_STACK_SIZE` would have charged every kernel task in
            // the system for this one task's cascade. `[u7stk]` rides the witness image, so the next
            // arc that grows a launcher watches `headroom=` shrink on the gate instead of on metal.
            const U7_LAUNCH_STACK_SIZE: usize = 32 * 1024;
            unaos_kernel::arch::sched::spawn_stack(
                "u7-launch",
                unaos_kernel::arch::syscall::u7_launcher,
                cpu,
                vcpu,
                U7_LAUNCH_STACK_SIZE,
            );
        }
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

    // BPACE: the whole ACPI phase — MADT topology discovery, the DMAR/IOMMU report and the PM-timer
    // liveness probe. Stamped HERE rather than immediately after `acpi::init` so the next stamp's
    // `d=` is the calibration alone and nothing else. x86-only, like the three calls it closes.
    #[cfg(target_arch = "x86_64")]
    unaos_kernel::bootpace::record("acpi");

    // 4b'''. Calibrate the TSC and the local-APIC timer against the PM timer, so tick-based timing
    // (scheduler sleeps, net RTO) and cycle-based busy-wait budgets become real wall-clock on this
    // machine's unknown Ivy Bridge crystal. Must precede SMP/scheduler bring-up so the APs inherit
    // the calibrated timer. No-op (fixed fallbacks) if the PM timer is absent.
    #[cfg(target_arch = "x86_64")]
    if let Some(pm) = unaos_kernel::arch::acpi::pm_timer(rsdp_addr) {
        unaos_kernel::arch::apic::calibrate(&pm);
    }

    // BPACE: calibration done — and, more importantly, the moment `counter_hz()` stops returning 0.
    // Every stamp BEFORE this one was still taken in raw counter ticks; they only become
    // milliseconds because the conversion happens at print time, downstream of this call.
    #[cfg(target_arch = "x86_64")]
    unaos_kernel::bootpace::record("calib");

    // 4c. SMP: start the application processors (INIT-SIPI-SIPI). Each AP brings up its own
    // per-CPU GDT/TSS + local APIC, then waits to enter its scheduler loop; the BSP continues to
    // drive everything below. `start_aps` also runs the post-bring-up SMP smoke test while the
    // APs are still idle.
    #[cfg(target_arch = "x86_64")]
    unaos_kernel::arch::smp::start_aps();

    // BPACE: application processors up (INIT-SIPI-SIPI + the post-bring-up smoke test).
    #[cfg(target_arch = "x86_64")]
    unaos_kernel::bootpace::record("smp");

    // 4d. Scheduler: now that SMP verification has run against idle APs, initialise the per-CPU
    // run queues, turn scheduling on, and spawn a small demo workload across the APs to exercise
    // preemption / cooperative yield / task exit. The BSP itself is never scheduled — it stays
    // the hardware-service core in the loop below.
    #[cfg(target_arch = "x86_64")]
    {
        unaos_kernel::arch::sched::init();

        // PULSE-NCPU (metal defect fix): turn scheduling on UNCONDITIONALLY — a quiet (non-witness,
        // non-sched_demo) GUI boot previously never called `enable()`, so the APs stayed parked in
        // `wait_and_run`, their busy/idle counters stayed frozen forever, and the vug/pulse CPU
        // meter honestly rendered 7 of 8 cores as PARKED dashes — reading on the panel as "1 CPU"
        // while a battery build's serial said "scheduling enabled on 7 AP(s)". Enabling is the
        // default-quiet law's shape — enable the feature, don't gate it behind a test knob: the APs
        // idle inside `run()` (sti;hlt), the idle counters tick, and pulse reflects every online
        // CPU. Prints nothing; the witness/demo paths below still call enable()/start_demo()
        // idempotently.
        unaos_kernel::arch::sched::enable();

        // U2 Part-0c: kernel-side boundary fixtures (no ring 3). Fire a self-NMI through the real
        // IPI path and confirm it was taken on the dedicated NMI IST stack (the honest B3 evidence),
        // and unit-exercise the canonical-`rcx` guard's refusal logic. Both need only the local APIC
        // + IDT/GDT (all up by now), not the scheduler, so they run here before the ring-3 demos.
        // DEFAULT-QUIET: re-proofs of metal-confirmed facts — behind the `witness` battery knob.
        #[cfg(feature = "witness")]
        {
            unaos_kernel::arch::syscall::nmi_self_fire();
            unaos_kernel::arch::syscall::canonical_guard_selftest();
        }

        // MIDDEN-M1 (witness battery): the x86 half of the shell-core fixture — see the aarch64
        // call site for what it proves. It needs no hardware at all (a synthetic volume), so it
        // sits with the other kernel-side boundary fixtures rather than with the storage probes.
        #[cfg(all(target_arch = "x86_64", feature = "witness"))]
        unaos_kernel::shell::midden_witness();

        // CLOCK-X1 (M3): the x86 wall-clock timebase witness — the SAMPLE half. Runs after
        // `apic::calibrate` (step 4b''') so the invariant TSC is calibrated; silent if this machine
        // has no invariant TSC. GR18 made it pay-as-you-go: it used to block here until it saw the
        // uptime second advance, which cost a uniform draw over the 1 Hz edge (18–976 ms measured
        // across eight metal boots) and WAS the whole `BPACE: sched d=` delta. It now samples and
        // returns; the verdict is delivered by `clock_x1_poll()` from the first service pass, which
        // is already seconds past the edge. See bootpace.md §8e.
        unaos_kernel::arch::syscall::clock_x1_witness();

        // LOGWIT-1 (witness + logts): the CLOCK-2b tap prefix's own fixture. Every other timestamped
        // line on a bench capture is only as trustworthy as the claim that the prefix reaches the FTDI
        // ring — the rMBP has no 16550, so the cable's ring IS the evidence, and the only thing that
        // ever attested to the tap was that it compiled. This emits a nonce marker, reads the capture
        // ring back through `ftdi::peek_recent`, and asserts the marker returned wearing a well-formed
        // 12-column prefix; it rejects an unprefixed and a malformed copy of its own marker first, so a
        // vacuous matcher reports FORBID rather than a green PASS. Runs HERE because the prefix's
        // monotonic form needs `calib` (step 4b''') behind it — earlier and the honest reading would
        // be the `?ms` form, which passes but proves less.
        #[cfg(all(feature = "witness", feature = "logts"))]
        unaos_kernel::logts::logwit1();

        // SNTP-X86 GATE (witness battery): the deterministic x86 SNTP client battery — canned datagrams
        // through the shared parser + the `crate::clock` anchor path, no NIC/network required. Proves x86
        // SNTP correctness under `./arroyo test` in any environment (the live boot sync in `service_net`
        // stays honest-but-INCOMPLETE under hermetic slirp). Prints `:: SNTP-X86-GATE: ... PASS [w=0x1f] ::`.
        #[cfg(all(target_arch = "x86_64", feature = "witness", feature = "smolnet"))]
        unaos_kernel::smolnet::sntp_x86_gate();

        // SOCK-8 GATE (witness battery): the deterministic x86 DNS client battery — canned datagrams
        // through the shared `crate::net_dns` parser, no NIC/network required. Proves x86 DNS parsing
        // (well-formed A / truncated / compression-loop / rcode) under `./arroyo test` in any environment
        // (the live boot resolve in `service_net` stays a bonus). Prints `:: DNS-X86-GATE: ... PASS [w=0xf] ::`.
        #[cfg(all(target_arch = "x86_64", feature = "witness", feature = "smolnet"))]
        unaos_kernel::smolnet::dns_x86_gate();

        // U1a: x86 ring-3 round-trip (the aarch64 M6a equivalent). Turn scheduling on (the default
        // test build never enables the feature-gated demo below, so the APs would otherwise idle in
        // `wait_and_run`), map the ring-3 window, then drop a scheduled task to ring 3 on an AP: it
        // runs an embedded routine that does `sys_write("hello from ring 3\n")` then `sys_exit(0)`
        // via SYSCALL/SYSRET, and the scheduler reclaims it. The BSP then waits (bounded) for the
        // round-trip and prints the verdict itself — see `await_verdict`: BSP-quiet so the AP's
        // SYSCALL/hello lines reach the (serial-less) framebuffer console uncontended and the demo
        // lands contiguously in the photographed boot log. All synchronous + QEMU-verifiable; metal
        // verification is a later arc boundary.
        // DEFAULT-QUIET: the whole U1a/U1b/U2-0a/U3/U3.5 ring-3 fixture flow re-proves metal-confirmed
        // facts every boot — behind the `witness` battery knob. (`sched::enable()` lives inside this
        // block; the opt-in `sched_demo` path self-enables via `start_demo`, so a quiet default boot
        // simply leaves the APs idle.)
        #[cfg(feature = "witness")]
        {
            let online = unaos_kernel::arch::smp::online_aps();
            // WITCORE: ask the placement module, not `.first()`. This block runs BEFORE the SCHED-X86
            // handoff publishes a split, so `worker_cpu(0)` resolves to `online_aps()[0]` — identical
            // to the old `.first()` — but the rule is now stated in exactly one place, and a later
            // reordering that moved this block after the handoff would stop aiming at the render core
            // instead of silently landing on it.
            if let Some(cpu) = unaos_kernel::arch::smp::worker_cpu(0) {
                unaos_kernel::arch::sched::enable();
                let demo = unaos_kernel::arch::syscall::setup();
                serial_println!(":: U1a: ring-3 demo — user task on core {} ::", cpu);
                unaos_kernel::arch::sched::spawn_user("u1a-hello", demo.hello, demo.sp, cpu);
                unaos_kernel::arch::syscall::await_verdict();

                // U1b: fault isolation. Drop three fault fixtures to ring 3 on the same core — each
                // provokes a specific fault (write to a kernel VA, write to the RO code page, exec
                // from the NX stack) the fault handler must answer with a task-KILL, not a kernel
                // halt. Ring 3 is cooperative (IF-masked), so they run FIFO to death one after
                // another; the BSP then waits (bounded, BSP-quiet) and prints the verdict. This is
                // the x86 mirror of aarch64 M6b.
                serial_println!(
                    ":: U1b: fault-isolation demo — 3 fault fixtures on core {} ::",
                    cpu
                );
                unaos_kernel::arch::sched::spawn_user("u1b-wild-write", demo.wild_write, demo.sp, cpu);
                unaos_kernel::arch::sched::spawn_user("u1b-code-write", demo.code_write, demo.sp, cpu);
                unaos_kernel::arch::sched::spawn_user("u1b-stack-exec", demo.stack_exec, demo.sp, cpu);
                unaos_kernel::arch::syscall::await_u1b_verdict();

                // U2 Part-0a: the TF+SYSCALL DoS fixture (becomes live the moment ring 3 can run
                // arbitrary loaded code). A ring-3 program arms `RFLAGS.TF` then `SYSCALL`; the
                // pending single-step #DB lands on the syscall-entry stub at CPL 0 (or, on some
                // platforms, in ring 3). The dedicated #DB IST + resume-or-kill policy neutralizes
                // it — the kernel must NOT halt. Runs after the U1b verdict so its (resumed/killed)
                // accounting can't perturb the U1b split; BSP-quiet verdict, kernel alive after.
                serial_println!(":: U2-0a: TF+SYSCALL DoS fixture on core {} ::", cpu);
                unaos_kernel::arch::sched::spawn_user("u2-tf-syscall", demo.tf_syscall, demo.sp, cpu);
                unaos_kernel::arch::syscall::await_u2_0a_verdict();

                // U3: per-process address spaces (CR3) — the x86 mirror of aarch64 M6d. First the
                // deterministic kernel isolation probe (two slots, same VA, distinct sentinels, swap
                // CR3 and read — no ring 3), then two ring-3 tasks each in its OWN address space, each
                // reading its slot-private sentinel — exercises the CR3 dispatch + teardown end to end.
                unaos_kernel::arch::syscall::u3_probe_once();
                unaos_kernel::arch::syscall::u3_run_fixture(cpu);

                // U3.5: preemptible ring 3 (the x86 twin of aarch64 M6e) — completes U3. Drop a
                // PREEMPTIBLE ring-3 spinner (never syscalls) plus a kernel co-task on the same core:
                // the timer evicts the spinner so the co-task runs (the DoS fix), and the spinner's
                // private-CR3 counter keeps climbing across preemptions (correct resume through the
                // CR3-at-dispatch path). A watchdog reaps the spinner via the scheduler. Runs LAST so
                // the preemptible task can't perturb the cooperative U1a/U1b/U2/U3 ordering above.
                unaos_kernel::arch::syscall::u3_5_run_fixture(cpu);

                // SERWIT-1: the serial transport's own fixture — the one gate that is about the
                // INSTRUMENT rather than the thing being instrumented. Every other `PASS` on this wire
                // is only as trustworthy as the wire, and the wire used to drop lines silently under
                // contention (`_print`'s `try_lock` failure branch discarded the line, with no counter
                // anywhere in the tree to notice). One worker per online AP, all released together so
                // their bursts genuinely overlap, then the BSP asserts the conservation law
                // `submitted == emitted + dropped + in_flight` with `dropped == 0`. The `[serwit]`
                // lines are sequence-numbered so the assertion can be falsified from the log itself
                // (`awk '/\[serwit\]/' target/serial.log | wc -l` == cores x burst) — a counter that
                // only ever agreed with itself would prove nothing.
                serwit1_run(&online);
            } else {
                serial_println!(":: U1a: no application processors online — ring-3 demo SKIPPED ::");
            }
        }

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

    // BPACE (M4): scheduler bring-up returned — per-CPU run queues, `sched::enable()`, the
    // CLOCK-X1 witness, and (on a `witness` build only) the whole ring-3 fixture flow. On the quiet
    // build that reaches metal this is init+enable+clock witness and nothing else, so `d=` here is
    // expected to be small; it exists so that "small" is a MEASUREMENT rather than an assumption.
    #[cfg(target_arch = "x86_64")]
    unaos_kernel::bootpace::record("sched");

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
    // BOT-PARK: the USB retry ladder's global-floor arithmetic, checked on every boot of every
    // arch. Deliberately OUTSIDE the `skip_xhci` split below: it needs no controller, and the Pi
    // kernel8 battery — the gate that has to hold this line — is a `skip_xhci` build.
    unaos_kernel::drivers::xhci::bot_park_selftest();
    #[cfg(not(feature = "skip_xhci"))]
    unaos_kernel::arch::pci::init(dtb_addr, dtb_size);
    // BPACE: PCI scan + xHCI controller bring-up returned. This is the SYNCHRONOUS half of USB; the
    // per-port enumeration that follows is asynchronous and stamps itself from the main loop. On a
    // `skip_xhci` build this tag is absent — the asymmetry the doc's "did not run (b)" reading uses.
    #[cfg(not(feature = "skip_xhci"))]
    unaos_kernel::bootpace::record("pci-usb");
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
            4 => "headless (no GOP protocol)",
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
    //
    // USBDBG-INVERT — **THIS BLOCK IS NO LONGER THE TERMINAL STATE OF AN x86 DESKTOP BUILD**, and the
    // cfg above is where that inversion is spelled. Peter's ruling (2026-08-19): *a diagnosis card
    // must be THE REAL DESKTOP plus instruments, not a parallel half-desktop.* The evidence is three
    // patches deep and each one only bought back a piece of the desktop this loop had replaced —
    // USBDBG-CURSOR gave it an arrow, USBDBG-ROUTE gave it clicks, and metal boot 6 then found the
    // shell window greyed and dead because the console SERVICE lives in the GUI loop this loop never
    // reaches. There is no end to that list: it is the whole of `x86_render_service`, re-implemented
    // one incident at a time. So on `x86_64` + `wc` the block is COMPILED OUT and the boot proceeds
    // into the ordinary GUI takeover — the same SCHED-X86 handoff a non-usbdebug build takes — with
    // the debug capabilities riding INSIDE it behind the knob:
    //
    //   * the per-pass services this loop uniquely ran now have a home on the normal path (see the
    //     service-mapping notes at `x86_usb_pump`; every one of them was already there except the
    //     boot-milestone re-dump, whose gate this arc widened to `witness OR usbdebug`);
    //   * the `USB-DEBUG:` event lines ride the real drains — `usbdebug_event_print` is called from
    //     `x86_render_service` and from the inline BSP GUI loop, keyed on the RAW report and printed
    //     BEFORE routing, so the card prints AND routes rather than printing INSTEAD of routing;
    //   * the `PTR:` press witnesses were never this loop's: they print from inside the EHCI HID
    //     service (`drivers/ehci`), which every x86 path polls, so they are loop-independent;
    //   * `USBDBG-INVERT` itself prints at the GUI takeover, so the wire names the regime.
    //
    // WHAT STILL COMPILES THIS BLOCK, and why the gate is the BUILD and not `desktop_uefi::is_active()`:
    // `usbdebug` WITHOUT `wc` (the knob's original purpose — pre-GUI bring-up on a card with no
    // compositor at all) and every aarch64 usbdebug build. Those regimes keep the print-only view
    // BYTE-FOR-BYTE, which is why the loop body below is otherwise untouched. A runtime gate was
    // considered and rejected: `desktop_uefi::activate` has exactly one caller (the Kepler takeover), so a
    // runtime test would make QEMU — where no Kepler exists — take the terminal loop, and the
    // inversion's own falsifier is a headless `UNAOS_USBDEBUG=1 UNAOS_WC=1` run reaching the GUI
    // selftests. A build that asks for the desktop gets the desktop, on metal and in QEMU alike.
    #[cfg(all(feature = "usbdebug", not(all(target_arch = "x86_64", feature = "wc"))))]
    {
        // VPERF M3: attach fbcon's cached-RAM shadow at the post-heap seam. From here the console
        // scrolls in cached RAM and the framebuffer only receives write-only blits — the
        // uncached-VRAM-read scroll (the rMBP's "nightmarishly slow" text output) is gone. This
        // is the LATE-ATTACH site by design: fbcon initialises pre-heap, and GUI builds never
        // reach this call (they detach fbcon instead; the Screen back buffer owns the heap
        // budget, so a second ~28 MiB shadow there would OOM the 48 MiB heap on metal).
        #[cfg(target_arch = "x86_64")]
        unaos_kernel::video::fbcon::attach_shadow();
        // Clear the boot spam so the (post-boot) hot-plug enumeration + live input own the screen.
        unaos_kernel::video::fbcon::clear();
        serial_println!(":: ============== USB DEBUG MODE ============== ::");
        serial_println!(":: Enumerating USB. Plug in a stick / keyboard / mouse, then type or move the mouse. ::");
        serial_println!(":: Watch for: 'MISSION SUCCESS' (storage), 'POINTER ... ABSOLUTE/RELATIVE', 'KEY', and the USB-DEBUG lines below. ::");
        loop {
            // WEDGE-8 (F3): a claimed LOAN — the synchronous BOT work below runs with no lock
            // held, so nothing that spins on the controller can ever wait out a preempted holder.
            // Busy (another context has the loan) skips this pass; the next one is milliseconds out.
            if let Ok(mut xhci) = unaos_kernel::drivers::xhci::claim() {
                xhci.poll_events();
                // BOOTPACE M2 — CONSOLE-FIRST: `service_ftdi` runs AHEAD of `service_storage`, so on
                // the pass that finally releases the deferred SCSI bring-up the console has already
                // armed and every line of that multi-second chain rides the live wire instead of the
                // FTDI capture ring's drop-oldest replay. Ordering only; both hooks are idempotent
                // no-ops when their work is not pending.
                xhci.service_ftdi();
                xhci.service_storage();
                xhci.service_hubs();
                xhci.service_hid_setproto();
                xhci.service_slot_disposal();
                xhci.service_enum();
            }
            // EHCI-3 (x86, ehcihid knob): poll the EHCI HID interrupt endpoints — the internal
            // rMBP keyboard/trackpad path. Same polled-service spot as the xHCI hooks above.
            #[cfg(all(target_arch = "x86_64", feature = "ehcihid"))]
            unaos_kernel::drivers::ehci::service_ehci_hid();
            // KEYREPEAT-X86: synthesise a held key's repeat before this pass's drain below.
            #[cfg(target_arch = "x86_64")]
            x86_typematic_pump();
            // BATMON-1 (x86, smc knob): refresh the battery snapshot (throttled internally to ~1 s)
            // and emit the `:: SMC-BATT: ... ::` witness. This is the serial-less metal-sitting view
            // (fbcon mirrors serial to the screen), so the battery readout is on-screen here too.
            #[cfg(all(target_arch = "x86_64", feature = "smc"))]
            unaos_kernel::drivers::smc::battery::refresh_if_due();
            // STOR-1 (x86, irqstorage knob): storage service task + `bx-blockreq` self-test, so the
            // interrupt-driven path is exercised on the serial-less metal boot too. One-shot, gated on
            // storage; a no-op without the knob.
            #[cfg(all(target_arch = "x86_64", feature = "irqstorage"))]
            {
                unaos_kernel::drivers::xhci::irqstorage::start_service_once();
                unaos_kernel::drivers::xhci::irqstorage::selftest_once();
            }
            // Once storage is up, mount + log the FAT volume geometry (one-shot).
            unaos_kernel::fs::fat::probe_once();
            // BT-BOND M1 (holocron knob): the classed-record store's whole main-loop presence — one call,
            // here, beside `probe_once` and for the same reason that one is here rather than in a driver: the
            // deferred write MUST run with no driver lock held. The Bluetooth chain that produces the record
            // this store exists for runs inside `service_ehci_hid()` holding `EHCI_HID`, and the writable FAT
            // volume rides USB mass storage serviced from that same pass — so a filesystem write issued from
            // in there would contend the xHCI storage loan from inside the EHCI service pass AND hold the
            // internal keyboard and trackpad hostage for its duration. The producer marks RAM dirty under the
            // lock; THIS call site does the I/O. `flush_if_dirty` re-checks that invariant rather than trusting
            // the placement (it DEFERS while `EHCI_HID` is busy — it does not refuse; see HCR1), so a call site that gets it wrong
            // goes loud instead of wedging. One-shot inside: the pure fixtures fire on the first pass, the load
            // latches once storage is up, and the flush costs a relaxed atomic load per iteration thereafter.
            // Like `fatverb_storage_witness`, it sits at ALL THREE storage-ready passes this file carries,
            // because which pass a given build reaches depends on its knobs.
            #[cfg(feature = "holocron")]
            unaos_kernel::fs::holocron::service();
            // PRTSCR: perform a pending Print Screen capture (`video::prtscr`). Here, beside
            // `probe_once` and `holocron::service`, and for the SAME reason those are here rather
            // than in a driver: the HID decoders detect the key edge while holding their controller
            // lock, and the writable FAT volume rides USB mass storage serviced from that same pass,
            // so an encode-and-write issued from in there would contend the storage loan from inside
            // the input pass and hold the internal keyboard and trackpad hostage for seconds. The
            // decoder sets a flag; THIS call site does the work. Ungated and arch-neutral, at all
            // THREE storage-ready passes this file carries, because which pass a given build reaches
            // depends on its knobs. Idle cost: one relaxed atomic load per iteration.
            unaos_kernel::video::prtscr::service();
            // PRTSCR-ST (prtscrst knob, default OFF): the capture's BOOT-TIME-WRITE witness — one
            // real capture through the same `capture()` the verb and the key call, then a read-back
            // of what landed on the medium. Here for the same reason `service` is here: it writes the
            // FAT volume and must do so with no driver lock held. It latches only on a pass that HAD
            // a volume, so an early pass before storage enumerates does not burn the one attempt.
            #[cfg(feature = "prtscrst")]
            unaos_kernel::video::prtscr::selftest_once();
            // SELFHOST-2 (x86, selfhost knob): verify the medium's own SRC.TGZ against SRC.SHA and
            // walk the tar, one-shot. It belongs beside `probe_once` for the same reason that one
            // does: it needs storage up and the volume lock free. Read-only throughout. Like the
            // FATVERB witness below, it sits at ALL THREE storage-ready passes — which one a given
            // x86 build reaches depends on its knobs, and this pass is the usbdebug one — with the
            // latch inside making it speak exactly once.
            #[cfg(all(target_arch = "x86_64", feature = "selfhost"))]
            unaos_kernel::selfhost::verify_source_once();
            // SDHC-4b (x86, sdhcblk knob): once `sdhc::bring_up` has registered the INTERNAL SD card
            // under its own block handle, mount it READ-ONLY and emit the witness (one-shot). Runs
            // here rather than in the bring-up so the card lock is released — the same reason
            // `probe_once` and `piusb27_service` run from the loop. Reads only: it is not a FAT writer.
            #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
            unaos_kernel::fs::fat::sdhc_probe_once();
            // FATVERB: the shell's storage witness — the read verbs, the exec probe and the write
            // gate must all name the same handle, and a write verb must consult the gate before it
            // mutates. One-shot. It MUST run here and not with the other shell fixtures at step 5:
            // the adoption review's capture showed those legs firing before `pci::init` and before
            // the USB publish, so they read `handles=global=absent sdhc=absent` and passed on
            // all-false inputs. Placed after the two probes above, the census names real handles
            // and the legs have something to be wrong about. It drives real verbs against a
            // throwaway console and mutates nothing (its write leg targets an unresolvable name).
            #[cfg(all(target_arch = "x86_64", feature = "witness"))]
            unaos_kernel::shell::fatverb_storage_witness();
            // WIFI-1 (wifi knob): the Broadcom/bcma firmware-load path — see the note at the second
            // loop site. Sits at all THREE storage-ready passes, like `fatverb_storage_witness`,
            // because which pass a given x86 build reaches depends on its knobs; the forward-only
            // state machine inside makes it speak exactly once. Read-only in arc 1.
            #[cfg(all(target_arch = "x86_64", feature = "wifi"))]
            unaos_kernel::wifi::service();
            // PIUSB-27: on the USB storage-ready edge, mount the stick's FAT volume read-only under
            // /fs/usb and emit the witness (aarch64 Pi path; runs with the xHCI lock released).
            #[cfg(target_arch = "aarch64")]
            unaos_kernel::fs::fat::piusb27_service();
            // GUI-WITNESS M3: re-dump the boot-milestone ring to serial on growth. A usbdebug-class
            // run surfaces the exact recorder ring via serial (M3 proof path), including the FTDI/block
            // milestones recorded from inside this loop. Serial-only + bounded.
            unaos_kernel::bootlog::service_serial_dump();
            // BPACE: re-emit the boot-phase timing ledger whenever it grows. Ungated, deliberately —
            // see `bootpace::service_dump`. Cheap when idle (one snapshot + one length compare).
            unaos_kernel::bootpace::service_dump();
            // FBCON-PACE: retire any console damage the pacing gate is holding. THIS LANE NEVER
            // DETACHES (see the U2 note below — the usbdebug view keeps fbcon attached on purpose),
            // so `fbcon::detach`'s sync flush is unreachable here and a burst that stops mid-frame
            // would otherwise leave its last band owed until the next print — after boot, until the
            // operator types. The call is PACED, not forced: it can only move a present earlier
            // within the frame it was already going to happen in, never add one. Free on a clean
            // ledger, and a no-op on every build where the console is not routed into a window.
            unaos_kernel::video::fbcon::console_service();
            // FLIGHT-RECORDER (x86): flush the captured serial boot log to UNAOS.LOG (usbdebug metal
            // boot benefits from an on-disk log too). Gated on storage; throttled; never blocks boot.
            #[cfg(target_arch = "x86_64")]
            unaos_kernel::flight_recorder::service();
            // U2 (x86): also run the FAT loader HERE so its lines are VISIBLE on the serial-less
            // metal boot — the usbdebug view keeps fbcon attached (unlike the GUI loop, which detaches
            // it before U2 runs). Same one-shot gate; loads HELLO.BIN + prints `hello from disk` + the
            // U2 PASS line onto the framebuffer.
            #[cfg(all(target_arch = "x86_64", feature = "witness"))]
            unaos_kernel::arch::syscall::u2_probe_once();
            // U4x (x86): the process model — sys_spawn (returns a HANDLE) + sys_wait (reaps by handle).
            // One-shot, gated on storage like U2; it pre-stages HELLO.BIN here (IF=1) then runs a parent
            // that spawns + reaps 2 children by handle, plus an orphan whose sys_wait(0) -> -ECHILD.
            #[cfg(all(target_arch = "x86_64", feature = "witness"))]
            unaos_kernel::arch::syscall::u4x_probe_once();
            // U5x (x86): handles as CAPABILITIES — rights + the enforcement CHECK + grant/attenuate/revoke
            // + routed sys_write + teardown-clear. One-shot, gated on storage + after U4x; the fixture is
            // an inline blob (no FAT I/O).
            #[cfg(all(target_arch = "x86_64", feature = "witness"))]
            unaos_kernel::arch::syscall::u5x_probe_once();
            // U6x (x86): the general OBJECT TABLE — (kind, target, rights) descriptors + first-free
            // allocation for ALL kinds, killing U5x's fixed CONSOLE_FD pin. One-shot, gated on storage +
            // after U5x; a printing spawner both prints AND spawns 2 children off the reserved console index
            // (the case U5x couldn't serve), plus kernel-side File/Socket-kind resolves.
            #[cfg(all(target_arch = "x86_64", feature = "witness"))]
            unaos_kernel::arch::syscall::u6x_probe_once();
            // U6bx (x86): REAL File handles — SYS_OPEN mints a File capability from the BSP-staged set and
            // SYS_READ serves bytes through it gated by CAP_READ (the pi4 U6b twin; the staged source is
            // the honest x86 divergence — the IF-masked handler can't pump the hlt()-ing xHCI BOT read).
            // One-shot, gated on storage + after U6x.
            #[cfg(all(target_arch = "x86_64", feature = "witness"))]
            unaos_kernel::arch::syscall::u6bx_probe_once();
            unaos_kernel::drivers::xhci::log_summary_once();
            // FBCON-PACE: the console's present census, once, HERE — beside the xHCI summary, i.e.
            // after enumeration and after the boot burst the pacing gate reshapes, so the numbers
            // cover the burst. This is the only place it is emitted on the bench lane: the census
            // used to ride `fbcon::console_flush`, which this lane never calls (no detach), and a
            // flush that now runs once per service pass must not print once per service pass.
            unaos_kernel::video::fbcon::console_pace_census_once();
            // VPERF (videobench knob, x86 only): the deterministic scripted scroll scenario —
            // one-shot, fires after the one-shot fixtures above have gone quiet, so the screen
            // settles on the scenario tail (what the DONE-gate screendump compares).
            #[cfg(all(target_arch = "x86_64", feature = "videobench"))]
            unaos_kernel::video::vperf::scenario_tick();
            // USBDBG-INVERT postscript — the drain of the PRESERVED regime, and it is a VIEWER
            // again, nothing more. USBDBG-ROUTE's `usbdebug_route`/`usbdebug_route_tail` and
            // USBDBG-CURSOR's `usbdebug_cursor_service` stood exactly here; all three were compiled
            // only under `usbdebug` + `wc` + `x86_64`, which is precisely the regime the inversion
            // moved onto the real desktop, so their call sites (and the helpers, and the auto-hide
            // edge that partnered the cursor service) are gone with them. On the path that still
            // compiles this loop there is no compositor to route to and no sprite to move — the
            // three patches were runtime no-ops here already (`desktop_uefi::is_active()` is false with no
            // Kepler takeover), so this deletion is behaviour-preserving for it.
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

        unaos_kernel::video::init_panel(framebuffer_addr as usize, framebuffer_size, info);

        // WMRETILE — the READINESS RE-TILE. `wm::place` early-returns on `!is_ready()` and has only
        // window-lifecycle call sites (create/close/close_owner), so a window created before this
        // point is published unpositioned and at WMMINW's fit-UNBOUNDED birth scale, and nothing
        // ever revisits it. This is the missing fourth event: it gives every such row a real
        // `place_scale` and a real origin, then reclaims the boxes the re-tile abandoned.
        //
        // Placed AFTER `init_panel` so the whole video surface is published before the layout reads
        // it, and unconditional/un-gated for the same reason the WRITER seed at step 0a is: a row
        // laid out against no panel is a general defect, not a compositor one.
        //
        // A no-op on every boot with no pre-ready rows — one WRITER read and one table scan, then
        // return — which on x86 is EVERY boot, since step 0a seeds `WRITER` before anything can
        // mint a window. It is silent there too: the witness sits behind the same early return.
        unaos_kernel::video::wm::retile_on_ready();

        // UVUG-2: wire the SYS_FB_PRESENT seam to the real scan-out now that WRITER is initialized.
        // One registration call site; the hook centers a user program's presented off-screen surface
        // on the panel (see `video::screen::present_surface`). Same code on the baremetal and QEMU
        // kernel8 paths (both reach here with a live framebuffer). aarch64-only — the seam lives in
        // arch/aarch64/syscall.
        #[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
        unaos_kernel::arch::aarch64::syscall::register_fb_present_hook(
            unaos_kernel::video::screen::present_surface,
        );
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
            // INROUTE: `input_router_selftest` used to run HERE. It does not any more — see its own doc
            // comment and the call site up in the `start_aps` block. The claim this comment used to make
            // ("EVENT_QUEUE is empty and no user slot is live") was false at this point in the boot: the
            // whole M6b..U7 fixture cascade is already spawned and running on the APs, holding ASIDs 1-8.
            typematic_selftest(); // UVUG-6: prove the dropped-KeyUp wedge is closed (report-level + guards)
            inwedge_selftest(); // INWEDGE: prove the input router refuses a held panel lock instead of wedging the input core on it
            unaos_kernel::arch::serial::RX_READY.init(); // M5c: the RX-wake semaphore's waiter list
            if !desktop_firmware_activate_maybe() { unaos_kernel::video::fbcon::detach(); } pi_rast_demo_maybe(); // CONSWIN-PI: the DESKTOP-READY seam rides the detach line on the same zero-source-lines discipline PI-RAST established, and it GUARDS the detach — `desktop_firmware_activate_maybe` answers true only when the console is ROUTED into a compositor window, and a routed console does not write the panel (`fbcon::draw_fb` hands back the window surface), so the one thing the detach exists to guarantee — exactly one core writing the panel — is already true and skipping it leaves the console window LIVE instead of freezing it at the handoff. Knob-off it is `#[inline(always)] false`, so this folds to the bare `detach(); pi_rast_demo_maybe();` it has always been. // PI-RAST demo (no-op unless UNAOS_PIRAST=1) on the SAME source line as the detach it rides, so the wire-in adds ZERO source lines ahead of any panic Location — the pi knob-off byte-identity constraint (PI-V3D-1 bisect-proven). Helper defined at file tail; runs here because the panel is up, fbcon has just stopped mirroring, and the input/render service tasks below are not spawned yet (nothing else paints).
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
                // PI-UI-2: the 1 Hz status-strip refresh pulse. Co-located on the input core (a
                // once-a-second wake, off the render core's frame-pacing critical path). Timer-gated
                // like rx-backstop: its sleep_ticks nap needs the live timer IRQ to wake.
                unaos_kernel::arch::sched::spawn("status-tick", status_tick, 0, input_cpu);
                // PIUSB-26: the xHCI event pump on its own ~4 ms cadence (see `usb_pump`). Co-located
                // on the input core; timer-gated like the neighbours (its `sleep_ticks` nap needs the
                // live timer IRQ). In QEMU raspi4b (not spawned) the input task's poll-nap still pumps.
                // SCHED-PRIO: the HID-report path runs in the interactive SERVICE band. Its passes are
                // micro (see `[piusb26]` — a `poll_events` on an empty ring), it naps between them,
                // and it is the whole latency budget of a moving mouse: a pump pass that queues behind
                // a spinning vug is a pointer report that arrives late, which is the P73 symptom.
                unaos_kernel::arch::sched::spawn_prio(
                    "usb-pump",
                    usb_pump,
                    0,
                    input_cpu,
                    unaos_kernel::arch::sched::PRIO_SERVICE,
                );
            }
            // SCHED-PRIO — the two panel service tasks join the band (see `sched::PRIO_SERVICE` for
            // the policy, the non-starvation argument and the lock/inversion accounting).
            //
            //   * `input`  — the input ROUTER: drains the UART/HID source and posts into GUI_CHANNEL.
            //     Everything downstream of a keystroke or a pointer report is gated on this task
            //     getting the core.
            //   * `render` — the COMPOSITOR PASS OWNER: `Screen::flush` → `wm::service_damage`, the
            //     cursor bracket, and the deferred-erase queue's liveness guarantee. The triage
            //     measured its composites collapsing 0.99→0.43/s under a six-vug fleet and its
            //     WM-lock erase defers going 29%→76%; it was a round-robin peer of every one of those
            //     fleets, on a core they are also placed on.
            //
            // `rx-backstop`, `status-tick` and `orphan-reaper` deliberately STAY at PRIO_NORMAL: the
            // first two are coarse periodic pokes with no latency requirement (a late 4 Hz strip
            // sample is invisible), and the reaper is background block I/O. Elevating them would put
            // the strip's `format!` + full-width band fill ahead of the fleet for no panel benefit.
            unaos_kernel::arch::sched::spawn_prio(
                "input",
                input_service,
                0,
                input_cpu,
                unaos_kernel::arch::sched::PRIO_SERVICE,
            );
            unaos_kernel::arch::sched::spawn_prio(
                "render",
                render_service,
                0,
                render_cpu,
                unaos_kernel::arch::sched::PRIO_SERVICE,
            );
            // U11-M2b: the deferred-free REAPER used to be spawned HERE. It is not any more — it now goes up
            // in the `start_aps` block, before the EL0 fixture cascade that can orphan a chain (see the
            // U11-REAP ORDERING FIX comment at that call site for the deterministic FAIL this site caused).
            // The summary line below still reports its load-balanced (SCHED-3b) scheduling: by this point the
            // reaper has been placed, alongside the two panel services spawned just above.
            serial_println!(
                ":: INPUT on core {} + RENDER on core {} + orphan-reaper load-balanced scheduled (OS on its own scheduler; BSP idle) ::",
                input_cpu, render_cpu
            );
            // PIUSB-33: NOW run the DEFERRED half of the split — enumeration only, on the BOOT CORE, past
            // the GUI-task spawn above. The RC + xHCI hardware bring-up already ran EARLY in build_boot_info
            // (the P38-proven context; the RC APB read that P39/P40/P41 proved hard-stalls in this deferred
            // context is long done, `XHCI_READY` set). The input/render tasks are already live on the APs
            // (the panel is unblocked and painting), so `enumerate`'s heap-backed DMA-side walk (rings, RS=1,
            // port/HID/storage) runs here without freezing the panel — and it touches the xHCI BAR MMIO, not
            // the stalling RC APB, so the deferred context is safe for it. Runs once, publishes the xHCI
            // controller, and hands steady-state servicing to `usb_pump` (spawned above). QEMU census-skips.
            #[cfg(feature = "piusb")]
            unaos_kernel::arch::piusb::bringup_task(0);
            // SMP-BAL: after its boot duties the BSP joins the scheduler (steal-eligible kernel
            // tasks only land here; user/pinned never do) instead of parking in hlt_loop forever.
            unaos_kernel::arch::sched::run_bsp(0);
        }
    }

    // SCHED-X86 (x86_64): the same handoff, on this arch, for the same reason. The BSP hands the
    // panel to scheduled kernel services and then JOINS THE SCHEDULER instead of falling into the
    // inline GUI loop below — the loop that made `bg`/`run` place ring-3 tasks on a core which never
    // popped its run queue (metal: 2 BGRUN spawns, zero `SYS_WIN_CREATE`, zero `:: SYSCALL:`, both
    // kills burning the full KILL_CONFIRM_MS, `c0:0/0` beside `0/263` everywhere else).
    //
    // It must diverge HERE, before `let mut console` / `Screen::new` below: `x86_render_service`
    // builds its own ~28 MiB cached-RAM back buffer, and a second one on the BSP would OOM the 48 MiB
    // metal heap. Same placement, and the same reason, as the Pi's block above.
    //
    // Gated off under `rast`: the RAST-1 demo below drives the BSP's local `screen`, which does not
    // exist on this path. That knob keeps the inline loop.
    #[cfg(all(target_arch = "x86_64", not(feature = "rast")))]
    if framebuffer_addr != 0 {
        let online = unaos_kernel::arch::smp::online_aps();
        // Two DISTINCT cores or nothing. `XHCI_CONTROLLER` is a raw `spin::Mutex` and both the
        // service task and the render/shell task take it (the latter through `fat` block reads,
        // `pal::pump_and_poll` inside a full-screen app, and `usbinfo`). Kernel tasks are preempted
        // like any other, so two preemptible takers of a raw spinlock on ONE core deadlock it: the
        // spinner cannot yield, so the holder it displaced can never be redispatched. Cross-core the
        // same contention is bounded spin and progresses. Declining the handoff is the honest
        // fallback — the inline loop below still works, it is only unscheduled.
        let split = match (online.first(), online.last()) {
            (Some(&render_cpu), Some(&svc_cpu)) if render_cpu != svc_cpu => {
                Some((render_cpu, svc_cpu))
            }
            _ => None,
        };
        if let Some((render_cpu, svc_cpu)) = split {
            // WITCORE: publish the split BEFORE anything is spawned, and emit the placement witness.
            // Every other site that needs a core (`smp::worker_cpu` for the cooperative ring-3 fixture
            // ladder, `smp::xhci_worker_cpu` for preemptible `XHCI_CONTROLLER` takers) asks that module
            // rather than re-deriving `online_aps().first()` — which, since SCHED-X86, silently named
            // the RENDER core. This call is what makes those answers correct: it must precede the
            // spawns, and it precedes `fbcon::detach()` so the `:: SCHED-X86 PLACE: ... ::` line also
            // lands on the panel of the serial-less metal boot.
            unaos_kernel::arch::smp::publish_sched_split(render_cpu, svc_cpu);

            // Reserve the channel's waiter capacity on the BSP, before any task can block on it —
            // otherwise the first park would grow a `VecDeque` (and take the heap lock) inside the
            // path that is proven not to allocate. Must precede the spawns.
            GUI_CHANNEL_X86.init();

            // USBDBG-INVERT — the regime witness, printed at the takeover it exists to prove. Ahead
            // of the detach below, so the line also lands on the panel of a serial-less diagnosis
            // card. A no-op line on every build without the knob (the whole fn is cfg'd out).
            #[cfg(all(feature = "usbdebug", feature = "wc"))]
            usbdebug_invert_witness();
            // The handoff milestones, replicated from below in their existing order and fired BEFORE
            // any task can paint. HANDOFF-CLEAN: record + detach strictly before the first console
            // frame, so the milestone's on-panel leg draws on the PRE-GUI panel and nothing paints
            // behind the GUI afterwards.
            unaos_kernel::bootlog::record("gui:handoff");
            // BPACE: the desktop-up number — the one every boot-pace trim is measured against. It
            // must keep landing on this path or the ledger silently loses its terminal stamp.
            unaos_kernel::bootpace::record("gui");
            // The GUI now owns the screen — stop fbcon mirroring serial onto the framebuffer, so
            // exactly one core writes the panel (a panic re-attaches).
            unaos_kernel::video::fbcon::detach();

            // The Pi's order: device service, then input, then render. `arg` carries the core index
            // so each task's own dispatch witness names the core it actually woke up on rather than
            // the one we intended.
            unaos_kernel::arch::sched::spawn(
                "usb-pump",
                x86_usb_pump,
                svc_cpu,
                svc_cpu,
                unaos_kernel::arch::sched::PRIO_NORMAL,
            );
            unaos_kernel::arch::sched::spawn(
                "input",
                x86_input_service,
                svc_cpu,
                svc_cpu,
                unaos_kernel::arch::sched::PRIO_NORMAL,
            );
            // WCSER-REHOME: record where the render role LIVES, as distinct from where the split
            // published it. They are the same today and stop being the same the moment a steal
            // declares this core dead — see `render_rehome_service`.
            RENDER_ROLE_CPU_X86.store(render_cpu, core::sync::atomic::Ordering::Relaxed);
            unaos_kernel::arch::sched::spawn(
                "render",
                x86_render_service,
                render_cpu,
                render_cpu,
                unaos_kernel::arch::sched::PRIO_NORMAL,
            );
            serial_println!(
                ":: SCHED-X86: RENDER on core {} + INPUT/usb-pump on core {} ({} AP(s) dispatching) — OS on its own scheduler ::",
                render_cpu, svc_cpu, online.len()
            );
            // The BSP joins the scheduler. Diverges — nothing below this line runs on this path.
            unaos_kernel::arch::sched::run_bsp(0);
        }
        serial_println!(
            ":: SCHED-X86: {} AP(s) dispatching — the render/service split needs 2 distinct cores; GUI stays inline on the BSP ::",
            online.len()
        );
    }

    let mut console = unaos_kernel::console::Console::new();

    // Build the double-buffered screen over the framebuffer. FrameBuffer is Copy, so we take a
    // handle and release the WRITER lock immediately. All GUI drawing now goes to a cached-RAM
    // back buffer; render() flushes only the damaged region to the (slow) framebuffer.
    let front_fb = *unaos_kernel::video::WRITER.lock();
    let mut screen = unaos_kernel::video::Screen::new(front_fb);

    // RAST-1/RAST-TEGRA (x86/virt + aarch64/virt, `rast` knob): run the software-rasterizer cube demo
    // through the panel `Screen` (call-never-edit), then hand the panel back. QEMU-witnessable path —
    // GICv2 virt + ramfb reaches here (Orin panel is wired in `tegra_early_stop`). Byte-identical off.
    #[cfg(all(feature = "rast", any(target_arch = "x86_64", all(target_arch = "aarch64", not(feature = "pi"), not(feature = "tegra")))))] // `pi`/`tegra` are AARCH64 BOARD SELECTORS and may only subtract on aarch64. This is the virt/x86 arm of three; `tegra_rast_demo_maybe` and `pi_rast_demo_maybe` are the others and BOTH carry `target_arch = "aarch64"` — this one did not, which was the defect: a build that set `pi` on x86 silently lost the demo AND kept the inline loop the `not(rast)` arm above hands off for, strictly worse than either branch. Folded onto this line to stay line-neutral (panic `Location` records embed line numbers; PARITY §5.3).
    {
        unaos_kernel::video::fbcon::detach();
        #[cfg(feature = "rastmc")] unaos_kernel::rast_demo::run_mc(&mut screen); unaos_kernel::rast_demo::run(&mut screen); // RASTPORT: the multi-core rung runs FIRST (1-core baseline + frame-pipelined pass, both unpaced) so the paced visible spin stays the last panel content — the same ordering the Orin call site uses, for the same reason. On the SAME SOURCE LINE as the `run` call it rides, so the wire-in adds ZERO source lines ahead of any panic Location (the PI-V3D-1 byte-identity convention); with `rastmc` off this folds to the bare `run(&mut screen);` RAST-1 has always had. NOTE this whole block is reached ONLY because `rast` compiles the SCHED-X86 handoff out (the `not(feature = "rast")` gate above) — with the handoff live, `run_bsp` diverges and nothing here runs. Deliberately gated identically to the sibling call (`not(pi)`, `not(tegra)`) so the REDSORT `pi`-leak fix covers BOTH call sites when it lands, rather than one.
    }

    let mut pal = unaos_kernel::pal::TargetPal::new(&mut screen);

    // GUI-WITNESS: the last milestone — the GUI is about to take the panel. Record it BEFORE the
    // detach so the ring captures the exact moment fbcon stops mirroring serial to the screen; from
    // here the `bootlog` shell verb is the operator's only witness surface (serial is silent on GUI
    // builds and the boot log is now painted over).
    //
    // HANDOFF-CLEAN (metal defect fix): record + detach STRICTLY BEFORE the first console paint.
    // The old order painted the console, then recorded this milestone — whose QUIET-PANEL on-panel
    // leg drew a text line straight onto the front framebuffer OVER the just-rendered prompt (the
    // "flash of text that garbles the prompt" Peter saw at the end of boot). Now the milestone
    // paints on the pre-GUI panel, detach flips GUI_ACTIVE, and NOTHING may paint behind the GUI
    // after this point (a panic still re-attaches).
    //
    // USBDBG-INVERT — the regime witness on the OTHER x86 takeover: this inline path is what a card
    // with fewer than two dispatching APs falls into, and a diagnosis card is exactly the kind of
    // machine that can land here, so the line must not be exclusive to the scheduled handoff.
    #[cfg(all(target_arch = "x86_64", feature = "usbdebug", feature = "wc"))]
    usbdebug_invert_witness();
    unaos_kernel::bootlog::record("gui:handoff");
    // BPACE: the desktop-up number — the one the operator's stopwatch has been approximating. Every
    // trim this arc's successor considers is measured against `gui=` on the total line.
    unaos_kernel::bootpace::record("gui");

    // The GUI now owns the screen — stop fbcon mirroring serial output onto the framebuffer
    // (a panic re-enables it). Boot diagnostics up to this point stay on screen until the first
    // console frame below repaints.
    unaos_kernel::video::fbcon::detach();

    console.draw(&mut pal);
    pal.render();

    use unaos_kernel::pal::GneissPal;

    // CURSOR-HIDE: whether the console loop last drew the cursor, so the auto-hide transition
    // erases the sprite exactly once (the full-screen demos clear every frame and need no erase).
    let mut cursor_was_visible = false;

    loop {
        // Poll xHCI Controller, then run any deferred storage work (synchronous BOT
        // transactions run here, in a safe non-event context).
        // WEDGE-8 (F3): a claimed LOAN — the BOT work runs with no lock held; a Busy claim
        // (another context has the loan) skips this pass and the next frame retries.
        if let Ok(mut xhci) = unaos_kernel::drivers::xhci::claim() {
            xhci.poll_events();
            // BOOTPACE M2 — CONSOLE-FIRST: `service_ftdi` ahead of `service_storage`, so the console
            // is armed before the deferred SCSI bring-up (which `service_storage` now holds until the
            // enumeration queue drains) puts its multi-second chain on the wire. Ordering only.
            xhci.service_ftdi();
            xhci.service_storage();
            xhci.service_hubs();
            xhci.service_hid_setproto();
            xhci.service_slot_disposal();
            xhci.service_enum();
        }
        // EHCI-3 (x86, ehcihid knob): poll the EHCI HID interrupt endpoints (internal rMBP
        // keyboard/trackpad). Same polled-service spot as the xHCI hooks above.
        #[cfg(all(target_arch = "x86_64", feature = "ehcihid"))]
        unaos_kernel::drivers::ehci::service_ehci_hid();
        // KEYREPEAT-X86: synthesise a held key's repeat before this pass's drain below.
        #[cfg(target_arch = "x86_64")]
        x86_typematic_pump();

        // STOR-1 (x86, irqstorage knob): bring up the interrupt-driven storage service task once a
        // block device is present (before the storage fixtures submit through it), then run the
        // `bx-blockreq` self-test once. Both are one-shot + gated on storage; a no-op without the knob.
        #[cfg(all(target_arch = "x86_64", feature = "irqstorage"))]
        {
            unaos_kernel::drivers::xhci::irqstorage::start_service_once();
            unaos_kernel::drivers::xhci::irqstorage::selftest_once();
        }
        // Once storage is up, mount + log the FAT volume geometry (one-shot). Runs with the xHCI
        // lock released; read_block re-locks it briefly, so there is no nested-lock hazard.
        unaos_kernel::fs::fat::probe_once();
        // BT-BOND M1 (holocron knob): the classed-record store's whole main-loop presence — one call,
        // here, beside `probe_once` and for the same reason that one is here rather than in a driver: the
        // deferred write MUST run with no driver lock held. The Bluetooth chain that produces the record
        // this store exists for runs inside `service_ehci_hid()` holding `EHCI_HID`, and the writable FAT
        // volume rides USB mass storage serviced from that same pass — so a filesystem write issued from
        // in there would contend the xHCI storage loan from inside the EHCI service pass AND hold the
        // internal keyboard and trackpad hostage for its duration. The producer marks RAM dirty under the
        // lock; THIS call site does the I/O. `flush_if_dirty` re-checks that invariant rather than trusting
        // the placement (it DEFERS while `EHCI_HID` is busy — it does not refuse; see HCR1), so a call site that gets it wrong
        // goes loud instead of wedging. One-shot inside: the pure fixtures fire on the first pass, the load
        // latches once storage is up, and the flush costs a relaxed atomic load per iteration thereafter.
        // Like `fatverb_storage_witness`, it sits at ALL THREE storage-ready passes this file carries,
        // because which pass a given build reaches depends on its knobs.
        #[cfg(feature = "holocron")]
        unaos_kernel::fs::holocron::service();
        // PRTSCR: perform a pending Print Screen capture (`video::prtscr`). Here, beside
        // `probe_once` and `holocron::service`, and for the SAME reason those are here rather
        // than in a driver: the HID decoders detect the key edge while holding their controller
        // lock, and the writable FAT volume rides USB mass storage serviced from that same pass,
        // so an encode-and-write issued from in there would contend the storage loan from inside
        // the input pass and hold the internal keyboard and trackpad hostage for seconds. The
        // decoder sets a flag; THIS call site does the work. Ungated and arch-neutral, at all
        // THREE storage-ready passes this file carries, because which pass a given build reaches
        // depends on its knobs. Idle cost: one relaxed atomic load per iteration.
        unaos_kernel::video::prtscr::service();
        // PRTSCR-ST (prtscrst knob, default OFF): the capture's BOOT-TIME-WRITE witness — one
        // real capture through the same `capture()` the verb and the key call, then a read-back
        // of what landed on the medium. Here for the same reason `service` is here: it writes the
        // FAT volume and must do so with no driver lock held. It latches only on a pass that HAD
        // a volume, so an early pass before storage enumerates does not burn the one attempt.
        #[cfg(feature = "prtscrst")]
        unaos_kernel::video::prtscr::selftest_once();
        // SELFHOST-2 (x86, selfhost knob): the source-verify + tar walk, one-shot — see the note at
        // the first loop site. This is the pass a headless `test`/`test-fat` boot reaches.
        #[cfg(all(target_arch = "x86_64", feature = "selfhost"))]
        unaos_kernel::selfhost::verify_source_once();
        // SDHC-4b (x86, sdhcblk knob): mount the INTERNAL SD card READ-ONLY once it has registered
        // under its own block handle, and emit the witness (one-shot). See the note at the other loop
        // site: it reads only and never becomes a second x86 FAT mutator.
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        unaos_kernel::fs::fat::sdhc_probe_once();
        // FATVERB: the shell's storage witness (one-shot) — see the note at the first loop site.
        // This file carries THREE storage-ready passes and which one a given x86 build reaches
        // depends on its knobs, so the call sits at all three and the latch inside makes it speak
        // exactly once. Mutates nothing.
        #[cfg(all(target_arch = "x86_64", feature = "witness"))]
        unaos_kernel::shell::fatverb_storage_witness();
        // WIFI-1 (wifi knob): the Broadcom/bcma firmware-load path. Runs the PCI-config census once,
        // then — because the blob lives on the FAT volume the USB-storage device serves, which
        // enumerates asynchronously above — waits here for that device and stages the blob exactly
        // once. Forward-only and terminal: after one attempt it parks, so this costs a relaxed atomic
        // load per iteration for the rest of the boot. Read-only in arc 1 (no MMIO, no device write).
        // Like `fatverb_storage_witness` above, it sits at all THREE storage-ready passes this file
        // carries, because which pass a given x86 build reaches depends on its knobs.
        #[cfg(all(target_arch = "x86_64", feature = "wifi"))]
        unaos_kernel::wifi::service();
        // PIUSB-27: on the USB storage-ready edge, mount the stick's FAT volume read-only under /fs/usb
        // and emit the witness (aarch64 Pi path; runs here with the xHCI lock released, like probe_once).
        #[cfg(target_arch = "aarch64")]
        unaos_kernel::fs::fat::piusb27_service();
        // GUI-WITNESS M3 (witness knob): re-dump the boot-milestone ring to serial whenever it grows.
        // On QEMU (serial live) this makes the recorded ring — including the FTDI/block milestones that
        // land from inside this loop — verifiable in serial.log without keyboard input, the M3 proof
        // path. Absent from a real metal GUI build (not witness); there the `bootlog` shell verb reads
        // the same ring on-panel. Serial-only + bounded (one print per new milestone).
        // USBDBG-INVERT: `usbdebug` joins the gate here for the reason the scheduled pump's copy
        // gives — the terminal loop called this UNGATED, and an inverted card that falls into this
        // inline loop (fewer than two dispatching APs) must not lose the M3 proof path either.
        #[cfg(any(feature = "witness", feature = "usbdebug"))]
        unaos_kernel::bootlog::service_serial_dump();
        // BPACE: re-emit the boot-phase timing ledger whenever it grows. NOT under the `witness`
        // gate above, and that difference is the point: the media `./arroyo esp-x86` writes carries
        // neither `witness` nor `usbdebug`, so a gated ledger would be absent from the only build
        // that ever reaches the bench. The GUI loop is where the late tags (`stor-*`, `fat-mount`,
        // `fr-flush`, `ftdi-up`) land, and where the last full block is emitted onto the live wire.
        unaos_kernel::bootpace::service_dump();
        // FLIGHT-RECORDER (x86): flush the captured serial boot log to UNAOS.LOG on the FAT volume so
        // a consumer who booted the vm-image (no serial capture) can copy the log off afterward.
        // Gated on storage internally; re-flushes on growth, throttled; never blocks boot.
        #[cfg(target_arch = "x86_64")]
        unaos_kernel::flight_recorder::service();
        // WITSWEEP (SERWIT-2 reachability): on x86 the mirror-tap announcement + one-shot verdict ride
        // `flight_recorder::service()` (its first statement). That function is x86-only, so on aarch64
        // the whole `[mirror]`/`:: SERWIT-2 ::` block never reached the wire and the TSTE tap-drop
        // counters were write-only. Mirror the x86 placement here: this loop iteration is an IRQs-
        // unmasked, no-locks-held, non-print context, exactly the contract `mirror_service` states.
        // Always-on — SERWIT is law, not a knob. (The aarch64 BAREMETAL builds never reach this loop —
        // the scheduler path owns them; their call rides `pump_usb_into_gui`, see there.)
        #[cfg(target_arch = "aarch64")]
        unaos_kernel::serial_ring::mirror_service();
        // U2 (x86): once a block device is present, load HELLO.BIN off the FAT volume and run it in
        // ring 3 (one-shot, gated like probe_once). Must live HERE, in the main loop — not with the
        // pre-xHCI U1a/U1b demo — because `fat::mount()` needs the usb-storage block device that
        // enumerates asynchronously above. No-op on aarch64 / when no FAT volume is present.
        #[cfg(all(target_arch = "x86_64", feature = "witness"))]
        unaos_kernel::arch::syscall::u2_probe_once();
        // U4x (x86): the process model — sys_spawn (returns a HANDLE) + sys_wait (reaps by handle).
        // One-shot, gated on storage like U2; pre-stages HELLO.BIN here (IF=1 — the syscall handler is
        // IF-masked and the xHCI BOT pump hlt()s), then runs a parent that spawns + reaps 2 children by
        // handle, plus an orphan whose sys_wait(0) -> -ECHILD (per-process handle tables).
        #[cfg(all(target_arch = "x86_64", feature = "witness"))]
        unaos_kernel::arch::syscall::u4x_probe_once();
        // U5x (x86): handles as CAPABILITIES — rights + the enforcement CHECK + grant/attenuate/revoke +
        // routed sys_write + teardown-clear. One-shot, gated on storage + after U4x; inline-blob fixture.
        #[cfg(all(target_arch = "x86_64", feature = "witness"))]
        unaos_kernel::arch::syscall::u5x_probe_once();
        // U6x (x86): the general OBJECT TABLE — (kind, target, rights) descriptors + first-free allocation
        // for ALL kinds, closing U5x's fixed CONSOLE_FD collision. One-shot, gated on storage + after U5x;
        // a printing spawner both prints AND spawns 2 children off the reserved console index (the case U5x
        // couldn't serve), plus a kernel-side File/Socket-kind + no-collision proof.
        #[cfg(all(target_arch = "x86_64", feature = "witness"))]
        unaos_kernel::arch::syscall::u6x_probe_once();
        // U6bx (x86): REAL File handles — SYS_OPEN mints a File capability from the BSP-staged set and
        // SYS_READ serves bytes through it gated by CAP_READ (the pi4 U6b twin; the staged source is the
        // honest x86 divergence — the IF-masked handler can't pump the hlt()-ing xHCI BOT read). One-shot,
        // gated on storage + after U6x.
        #[cfg(all(target_arch = "x86_64", feature = "witness"))]
        unaos_kernel::arch::syscall::u6bx_probe_once();
        // INSTALL-CORE (x86, installdemo knob): once the block device (the armed BLANK scratch disk
        // the builder attaches under UNAOS_INSTALLDEMO=1) is present, run the installer engine
        // end-to-end — blank-check, GPT write + parse-back verify, FAT32 format, copy-and-verify a
        // payload by re-reading every extent, the in-tree FAT read-back interop check, the negative
        // (1-byte-corruption-caught) test, and the post-write refusal guard — emitting the
        // `:: INSTALL: gpt+fat32+copy verify => PASS ::` witness. One-shot + gated on storage. No-op
        // (module absent) without the feature; never touches the boot ESP (a separate ide-hd).
        // INSTGUI supersedes the auto-probe: when the graphical installer is armed, the attended
        // Enter on its warning screen is the ONLY trigger — the engine must not fire on its own.
        #[cfg(all(target_arch = "x86_64", feature = "installdemo", not(feature = "instgui")))]
        unaos_kernel::install::install_probe_once();
        // INSTGUI: pick up disks that enumerate after the dialog opened (repaints only on change).
        #[cfg(all(target_arch = "x86_64", feature = "wc", feature = "instgui"))]
        unaos_kernel::video::instgui::service();
        // One-shot USB topology dump to serial (enumeration diagnosis; `usbinfo` shows it live).
        unaos_kernel::drivers::xhci::log_summary_once();

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
            // WINX-7 — offer each event to the focused user app's input ring before the shell sees it.
            // `user_input_route` returns the event UNCHANGED when no app took it, and `Unknown` (never
            // `None`) when a ring consumed it: `None` is this loop's end-of-queue sentinel, so
            // returning it for a routed keystroke would truncate the drain at the first one.
            //
            // This GUI loop is shared, but the router is x86-only — aarch64 routes EL0 input from its
            // own path in `arch/aarch64` and has no `arch::x86_64` module to name, so the call is
            // split rather than gated inside the callee.
            //
            // CLICK-X86 — and a pointer BUTTON is ADDRESSED before it is DELIVERED. `user_input_route`
            // routes by FOCUS: whatever it is handed goes to whoever holds the keyboard. That is right
            // for a keystroke and wrong for a click, which belongs to the window UNDER THE CURSOR, so
            // `wc_click_route` runs first and hit-tests the press. It answers `true` only when it has
            // CONSUMED the event (a press on kernel furniture or on the bare desktop, and the release
            // that follows one); otherwise the ordinary path continues underneath it, addressed to
            // whatever focus the router just left in place — which on a raise is the newly focused
            // window, so `user_input_route` pushes the press into the ring of the window that was
            // clicked. Non-`Button` events are returned untouched, so keys and motion are unaffected.
            //
            // `Event::Unknown` for a consumed click, and never `Event::None`: `None` is this loop's
            // end-of-queue sentinel, so returning it would truncate the drain at the first click.
            //
            // WC-TAB/x86 — and TAB is judged BEFORE either router, because it is addressed to neither.
            // It belongs to the window system itself: `wc_focus_key` is the one keystroke that moves
            // focus rather than being delivered under it, and it must be intercepted ahead of
            // `user_input_route` or the focused app swallows the only exit from its own window (Boot
            // AH: 165 focus grants, 164 revokes, `kill <pid>` unreachable for the rest of the boot).
            // Ahead of `wc_click_route` too — that call reads the cursor and hit-tests, work a key
            // event has no business paying for; it returns `false` for any non-`Button` regardless.
            //
            // ONE interception covers BOTH directions on this arch, which is the divergence from
            // aarch64 worth naming. This pair runs on every event whatever holds focus (the focus test
            // is inside `user_input_enqueue`), so app -> shell and shell -> window pass through the
            // same line; the Pi needs a second entry point on its shell drain only because its router
            // is reached solely while an app has focus.
            // WMDIRECT — `raw` is hoisted out of this block because a live title-bar DRAG has to be
            // steered by the report the HARDWARE sent, not by what routing left of it. See the drag
            // tick after the match below for the whole argument.
            #[cfg(target_arch = "x86_64")]
            let (raw, ev) = {
                let raw = pal.poll_event();
                // USBDBG-INVERT — the debug view, PRINTED AND THEN ROUTED. The terminal loop this
                // replaces printed INSTEAD of routing; here the instrument reads the raw report on
                // its way past and consumes nothing, so a `usbdebug` card and a stock card route
                // identically. Compiled out without the knob.
                #[cfg(all(feature = "usbdebug", feature = "wc"))]
                usbdebug_event_print(raw);
                (raw, unaos_kernel::arch::x86_64::syscall::wc_route_event(raw))
            };
            #[cfg(not(target_arch = "x86_64"))]
            let ev = pal.poll_event();
            match ev {
                unaos_kernel::pal::Event::None => break,
                unaos_kernel::pal::Event::Key(c) => {
                    had_event = true;
                    // INSTGUI — while the installer dialog is open it owns the keyboard;
                    // the console resumes the moment it closes.
                    #[cfg(all(target_arch = "x86_64", feature = "wc", feature = "instgui"))]
                    if unaos_kernel::video::instgui::consume_key(c) {
                        continue;
                    }
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
                    // CURSOR-SAVE-UNDER (metal defect fix: trails across midden): restore the
                    // pixels stashed under the sprite, move, then stash-and-draw at the new
                    // position. The console repaints nothing per frame, so the sprite must be
                    // fully self-undoing — the old flat-color erase punched grey boxes through
                    // text and missed the drop shadow's overhang, smearing motion.
                    //
                    // CURSOR-X86: on this target these three verbs now drive the COMPOSITOR SPRITE
                    // (`video::cursor`, front buffer, above the window layer) rather than a
                    // back-buffer sprite — `pal::cursor::SPRITE_OWNS_PAINT` carries the whole
                    // argument. The sequence is unchanged and still correct: `move_rel` repaints the
                    // arrow at the new position on the report itself, and `draw_over` is the
                    // idempotent tail. What changed is that the arrow no longer needs the
                    // `pal.render()` below to reach the glass, and no longer disappears under a
                    // window.
                    //
                    // CURSOR-WCR — and the leading `restore` is gone WHERE THE SPRITE OWNS THE PAINT,
                    // because there it was `video::cursor::undraw()` immediately in front of a
                    // `repaint` whose own `undraw_locked` performs the same restore. Two restores,
                    // one of them wasted, is the cheap half of the cost; the expensive halves are
                    // that the standalone `undraw` cleans WHOLE SCANLINES (`flush_box`) where the
                    // repaint's union cleans only the sprite's columns, that it publishes the panel
                    // with the arrow taken down and not yet put back — the intermediate publication
                    // CURSOR-10 exists to remove — and that it issues a second
                    // `damage_intersecting` per report. Kept unconditionally on the back-buffer
                    // targets, where `restore` really is the only thing that takes the sprite off
                    // and `move_rel` paints nothing.
                    #[cfg(not(target_arch = "x86_64"))]
                    unaos_kernel::pal::cursor::restore(&mut pal);
                    unaos_kernel::pal::cursor::move_rel(
                        x, y,
                        pal.width() as i32,
                        pal.height() as i32,
                    );
                    unaos_kernel::pal::cursor::draw_over(&mut pal);
                }
                unaos_kernel::pal::Event::MouseAbsolute { x, y } => {
                    had_event = true;
                    // CURSOR-SAVE-UNDER: absolute report (0..=32767 HID space), same sprite.
                    // CURSOR-WCR: leading restore dropped where the sprite owns the paint — see the
                    // relative arm above for the whole argument.
                    #[cfg(not(target_arch = "x86_64"))]
                    unaos_kernel::pal::cursor::restore(&mut pal);
                    unaos_kernel::pal::cursor::set_abs(
                        x, y,
                        pal.width() as i32,
                        pal.height() as i32,
                    );
                    unaos_kernel::pal::cursor::draw_over(&mut pal);
                }
                // CLICK-X86 — the PRESS arm. Before this arc the drain's `match` ended `_ => {}` with
                // no `Event::Button` arm at all: HID pushed presses onto the queue and this loop
                // popped and DISCARDED them, which is the whole of the "clicks get eaten" complaint on
                // this arch and the reason the "out-of-focus click stops my app" complaint could never
                // have had a mechanism either.
                //
                // Reaching this arm now means the press was NOT consumed by `wc_click_route` above and
                // NOT taken by a user ring — i.e. it is the SHELL's click. The shell on x86 has no
                // click model of its own yet (aarch64's `click1_dispatch` is that arch's scheduled
                // render service, which does not run here), so there is nothing to dispatch it to and
                // the arm is deliberately empty of policy. What it must do is count as activity, so
                // the loop presents this pass and does not `hlt` on a click; and the press is already
                // on the wire from the router's `[clickroute]` line, which is where the disposition is
                // recorded. When x86 grows a shell click model this is the one place it attaches.
                //
                // x86-gated: this loop is shared, and on aarch64 a Button still falls through the
                // catch-all exactly as it did (that arch routes clicks from its own drains), so this
                // arc leaves that target's behaviour untouched.
                #[cfg(target_arch = "x86_64")]
                unaos_kernel::pal::Event::Button(_mask) => {
                    had_event = true;
                }
                // Timer / Unknown: nothing to do.
                _ => {}
            }
            // WMDIRECT — **STEER A LIVE TITLE-BAR DRAG, OFF `raw`, AFTER THE ARMS ABOVE.**
            //
            // The predicate is the RAW report and NOT `ev`, and that distinction is the whole of this
            // line's correctness. `Event::Mouse`/`Event::MouseAbsolute` are PACKABLE, so whenever a
            // ring-3 app holds focus and its ring is not full, `user_input_route` consumes the report
            // and hands back `Event::Unknown` — it never reaches the pointer arms at all. A title-bar
            // grab GUARANTEES exactly that state, because the chrome arm of `wc_click_route_at` calls
            // `user_input_set_active(owner)` with the dragged window's own owner. Keyed on `ev`, the
            // drag would therefore be dead for every app window and alive only for the console and
            // the focus-exempt desktop row (whose arms take `set_active(0)`) — app-dependent,
            // nondeterministic (an app that stops draining fills its ring, the push fails, and the
            // drag springs to life mid-gesture), and with the release edge's unthrottled final
            // reposition TELEPORTING the window to wherever the hand let go.
            //
            // Placed after the `match` rather than beside the routers so that the cursor is FRESH on
            // both branches, which is the other half of it — `wc_drag_motion` reads the shared
            // `pal::cursor` position rather than the report's delta:
            //   * CONSUMED — `user_input_route` -> `pal::cursor::track_routed` has already applied it
            //     (CURSOR-VUG). Without that arc this line would steer to a stale position.
            //   * DECLINED — the `Mouse`/`MouseAbsolute` arms above have just applied it.
            // Exactly ONE tick per pointer report either way, so a relative report is never applied
            // twice and the 16 ms throttle measures real time rather than a doubled rate.
            //
            // Costs one `matches!` on non-pointer events and one atomic load when no drag is live,
            // which is every report on a boot where nobody grabbed a title bar.
            #[cfg(target_arch = "x86_64")]
            unaos_kernel::arch::x86_64::syscall::wc_route_tail(raw);
        }

        // CURSOR-HIDE: restore the pixels under the sprite once when the auto-hide delay
        // expires (reappearance is instant — the Mouse/MouseAbsolute arms above stamp the
        // activity clock before drawing).
        let cursor_vis = unaos_kernel::pal::cursor::visible();
        if cursor_was_visible && !cursor_vis {
            unaos_kernel::pal::cursor::restore(&mut pal);
        }
        cursor_was_visible = cursor_vis;

        // Nothing queued — sleep until the next interrupt (timer/xHCI) rather than busy-spin.
        if !had_event {
            unaos_kernel::hlt();
        }

        // Present this frame: flush the damaged region of the back buffer to the framebuffer.
        // No-op when nothing was drawn this iteration, so the idle (hlt) path stays cheap.
        pal.render();
    }
}

/// Jetson Orin Nano (Tegra234) platform bring-up — the `tegra` build's terminus. JM3 installs the
/// kernel's own MMU via `mmu_tegra::init` (RAM Normal-WB + the Tegra device window Device-nGnRE + a
/// fault vector), which is what lets the serial path drive UARTC. JM4 then brings up the Tegra234
/// GIC-600 + the ARM generic timer on the BOOT CORE and proves the timer PPI (INTID 30) delivers through
/// the GIC at EL2 (`verify_live`). **JM6** then drops the boot core **EL2 -> EL1** (`boot_tegra`, reusing
/// mmu_tegra's identity L1) and runs the full six-primitive M4 CAPSTONE cooperatively at EL1
/// (`sched::run_capstone_boot_core`) — the first time the scheduler runs on Orin silicon. Heap/memory
/// init and Orin *userspace* (EL0) are later arcs; SMP (PSCI CPU_ON) is parked (metal-blocked, see JM6/
/// JM5 notes at the call site), so JM6 is single-core and sidesteps that wall.
///
/// Diverges (so `kernel_main` is `!` on tegra and everything after the call site is unreachable —
/// covered by that fn's `allow(unreachable_code)`). `run_capstone_boot_core` never returns: it drains the
/// run queue (CAPSTONE 6/6) then idle-spins, so a headless serial capture gets the whole log. `tegra` is
/// off in every QEMU build, so this is never compiled into a regression run — its verdict is Orin metal.
#[cfg(all(feature = "tegra", target_arch = "aarch64"))]
fn tegra_early_stop(boot_info: &'static mut BootInfo) -> ! {
    // 1. Install the kernel's own MMU FIRST — SILENT. Nothing has printed yet (fbcon::init is
    //    print-free when fb_addr == 0), and the serial path cannot touch UARTC until this maps the
    //    Tegra device window. The FIRST serial byte of the whole kernel is the `mmu live` line below.
    let mmu = unaos_kernel::arch::mmu_tegra::init(boot_info); unaos_kernel::arch::serial::mark_mmio_ready(); // DARKWIN-GUARD arm — window mapped; see tegra_darkwin_witness (tail)
    serial_println!(
        ":: tegra: mmu live (EL{}) — RAM Normal-WB + Tegra Device-nGnRE mapped ::",
        mmu.el
    ); tegra_darkwin_witness(boot_info); // DARKWIN-GUARD witness + eaten-EDID re-emit (tail fn)
    serial_println!(
        ":: tegra: mmu regs — SCTLR {:#x}->{:#x} TCR={:#x} MAIR={:#x} TTBR0={:#x} RAM-GiB-mask={:#x} ::",
        mmu.sctlr_old,
        mmu.sctlr_new,
        mmu.tcr,
        mmu.mair,
        mmu.ttbr0,
        mmu.ram_gib_mask,
    );
    // XCARVE-3 (always-on correctness, one-shot witness): report the protected-carveout hole the MMU
    // just EXCLUDED (unmapped) from the cacheable map. Boot-21 proved PA 0x26b900000 is firewalled
    // carveout DRAM with no software writer — any cache traffic touching it raises the SNOC RAS — so the
    // window is left with no valid translation. Source names whether the DTB declared the reservation
    // (`DTB`) or we fell back to a bounded quirk (`QUIRK`). `hole_size == 0` ⇒ nothing excluded.
    if mmu.hole_size != 0 {
        // XCARVE-6: the excluded set is now the two QUIRK windows (0x26b9, 0xbe) plus every DTB
        // `/reserved-memory` carveout inside a RAM GiB. List EVERY window (no-silent-drop, XCARVE-5 law).
        serial_println!(
            ":: tegra: XCARVE-6 carveout exclusion — {} protected window(s) UNMAPPED from the cacheable map (SNOC-firewalled DRAM, no software writer) ::",
            mmu.hole_count,
        );
        for i in 0..mmu.hole_count {
            let (hb, hs, hsrc) = mmu.holes[i];
            if hs == 0 {
                continue;
            }
            let src = match hsrc {
                1 => "DTB /reserved-memory",
                // XCARVE-8: a QUIRK extent is a bounded GUESS (no readable register/DTB truth for the
                // undeclared windows) — say so in the banner so a refuting hit reads as "widen the guess".
                2 => "QUIRK (DTB silent; extent = bounded GUESS)",
                _ => "unknown",
            };
            serial_println!(
                ":: tegra:   window[{}] [{:#x},{:#x}) {} KiB @ GiB {} (source: {}) ::",
                i,
                hb,
                hb + hs,
                hs >> 10,
                hb >> 30,
                src,
            );
        }
        if mmu.hole_dropped != 0 {
            serial_println!(
                ":: tegra: XCARVE-6 WARNING — {} protected window(s) DROPPED (set/pool full); exclusion set INCOMPLETE — cacheable carveout may remain ::",
                mmu.hole_dropped,
            );
        }
    }
    // XCARVE-2 temporal bracket (RAS store-site hunt, `UNAOS_VUGRAS=1`): first bracket, right after the
    // MMU identity-maps RAM — the 0x26b900000 line is now cleanable. Heap/span-B are not carved yet, so
    // the span-B bisect witnesses "empty" here; the tripwire still fires. No-op knob-off.
    unaos_kernel::vugras::phase("post-mmu");
    // JB1f: install the full healed exceptions.rs vectors NOW — before fbcon starts mirroring —
    // retiring the unhealed early window the 2026-07-11 bench caught: the A78AE-1941500 phantom
    // struck fbcon's glyph loop (the boot's heaviest ifetch+store stretch) under mmu_tegra's
    // divergent Part-C probe-and-spin vectors, twice, ms after `panel LIVE`. From here every
    // EC=0-with-valid-D-side strike heals (`ic iallu` + retry) instead of dying; Part C now
    // covers only the silent MMU switch itself. Audited safe this early: the handler reads the
    // EL it booted at (BOOT_EL latches 2 here), serial is live (the banner above printed), the
    // fatal path busy-spins (timer::LIVE is still false), and the IRQ entries stay dormant —
    // install()'s HCR_EL2.AMO|IMO|FMO routing changes where a physical IRQ WOULD land, but DAIF
    // stays fully masked until JM4's enable_irq below, by which point the GIC is up.
    unaos_kernel::arch::exceptions::install(); #[cfg(feature = "orinwdt")] unaos_kernel::arch::wdt_tegra::boot_arm(); // ORIN-REBOOT: arm the TKE boot watchdog HERE — MMU device window live (the TKE is MMIO), serial live (the witness must land), vectors healed — so every later wedge (GIC, USB, panel, the drop) sits inside the covered window. Disarmed on the terminus line; knob-off the statement vanishes (APPENDED to this line, so no panic Location shifts).
    // JM7 (video): report the GOP the firmware handed off (addr=0 = headless boot, fbcon inert).
    // With a monitor connected, fbcon has been mirroring serial output onto this framebuffer since
    // kernel_main step 0 (under the UEFI map), and mmu_tegra just mapped its GiBs into BOTH tables
    // (the mask above includes them) — so the monitor is already showing this very boot log, and
    // keeps showing it across the EL1 drop.
    serial_println!(
        ":: tegra: JM7 — GOP fb addr={:#x} size={:#x} {}x{} stride={} bpp={} ::",
        boot_info.framebuffer_addr,
        boot_info.framebuffer_size,
        boot_info.framebuffer_info.width,
        boot_info.framebuffer_info.height,
        boot_info.framebuffer_info.stride,
        boot_info.framebuffer_info.bytes_per_pixel,
    );
    // JD1 (video): the JM7 GOP is BltOnly — no linear framebuffer — so the panel is dark even though
    // the firmware's DCE is still scanning out a live framebuffer from a DRAM carveout. INHERIT that
    // scanout (don't re-init the pipeline): the firmware's SIMPLEFB display-handoff published the
    // scanout base+geometry+format into the DTB (a `simple-framebuffer` node), so `jd1_survey` reads
    // it read-only (no display MMIO), maps the carveout Normal-WB into BOTH translation tables, paints
    // a test pattern, and brings fbcon online on it — from here every `serial_println!` (JB1a … JM4 …
    // and the EL1 CAPSTONE, across the JM6 drop) also paints onto the Orin panel. Headless (no handoff
    // published, or geometry fails sanity): `jd1_survey` returns None and this whole block is a no-op,
    // so the boot stays byte-identical to the pre-JD1 headless path. `tegra`-only → inert in QEMU.
    if let Some(fb) = unaos_kernel::arch::display_tegra::jd1_survey(
        boot_info.dtb_addr,
        boot_info.dtb_size,
        mmu.ram_gib_mask,
    ) {
        if unaos_kernel::arch::mmu_tegra::map_fb_region(fb.base, fb.len) {
            // Prove the inherited {base, stride, format} reach the panel before fbcon clears to black.
            unaos_kernel::arch::display_tegra::jd1_test_pattern(&fb);
            // fbcon online on the inherited scanout: fills black + starts mirroring the boot log. The
            // EL1 twin was patched by map_fb_region, and the tegra path never detaches fbcon (JM7), so
            // the mirror survives the JM6 EL2 -> EL1 drop and shows the CAPSTONE run live.
            unaos_kernel::video::fbcon::init(fb.base, fb.len, fb.info);
            // JD2: seed the shared GUI front-buffer handle with the same inherited scanout, so the
            // EL1 console pump (spawned below, alongside CAPSTONE) can build its double-buffered
            // `Screen` over it when the first keystroke arrives. fbcon and WRITER are handles to
            // the same physical framebuffer (the x86/pi pattern); until the console takes over,
            // only fbcon draws.
            unaos_kernel::video::WRITER.lock().init(fb.base as usize, fb.len, fb.info);
            // WMRETILE — the tegra twin of the readiness re-tile at step 3, and DEFENSIVE rather
            // than required. `tegra_early_stop` diverges before `kernel_main` step 3 ever runs, so
            // this is the only `WRITER` attachment the Orin boot reaches and the step-3 hook
            // cannot stand in for it — but nothing can mint a pre-ready row here TODAY either: the
            // heap and the scheduler both come up after this point, so the table is empty and the
            // call returns 0 on every current Orin boot. It is here so that the Orin path does not
            // silently regain the hole the moment window creation moves earlier than JD2, which is
            // exactly the direction that path is growing. Same no-op-when-empty terms; see
            // `wm::retile_on_ready`.
            unaos_kernel::video::wm::retile_on_ready();
            serial_println!(
                ":: tegra: JD1 — panel LIVE: inherited scanout mapped + fbcon mirroring the boot log ::"
            );
        } else {
            serial_println!(
                ":: tegra: JD1 — scanout base {:#x} not mappable (not DRAM GiB 2..63); headless ::",
                fb.base,
            );
        }
    }
    // JB1d: the A78AE erratum-1941500 probe/workaround (CPUECTLR_EL1[8]) — the EC=0 phantom's
    // leading suspect after the D-side read-back proved an I/D divergence. Prints MIDR + the bit
    // state BL31 left, sets it if clear. Runs before everything else so the whole boot (JB1b/c +
    // the drop + CAPSTONE) executes under the workaround.
    unaos_kernel::arch::mmu_tegra::a78ae_errata_probe();
    // JB1a: print the BPMP IPC geometry (shmem TX/RX, HSP mboxes, reserved-memory carveouts) from
    // the firmware DTB — a read-only RAM walk, no MMIO (the JX1 lesson: gated Tegra blocks are
    // EL3-fatal to touch, so the geometry gets VERIFIED off the firmware's own tree before the
    // JB1 IVC arc maps or touches anything).
    unaos_kernel::arch::fdt_tegra::jb1a_dump(
        boot_info.dtb_addr,
        boot_info.dtb_size,
        mmu.ram_gib_mask,
    );
    // JB1b: establish the BPMP IVC command channel and prove it with one MRQ_PING — the transport
    // every partition-ungate MRQ (XUSB, nvdisplay) rides on. Geometry is resolved from the same
    // DTB (never hardcoded); a missing/odd DTB shape prints and skips. Pre-drop, EL2, polled;
    // every new MMIO class prints before the first touch (the JX1 EL3-fatal discipline).
    //
    // `xusb_alive` gates the JB2b xHCI attach below: touching 0x0361_0000 without a completed
    // JB1c ungate is an EL3-fatal CBB abort (the JX1 lesson), so the attach runs ONLY on a boot
    // whose ungate proved the block alive.
    let mut xusb_alive = false;
    match unaos_kernel::arch::fdt_tegra::bpmp_geometry(
        boot_info.dtb_addr,
        boot_info.dtb_size,
        mmu.ram_gib_mask,
    ) {
        Some(geom) => {
            // JB1c: with the channel proven by the ping, ungate the XUSB host partition (power
            // domains -> clocks -> reset deassert, all ids read off the DTB's usb@3610000 node)
            // and re-probe the xHCI capability block that was EL3-fatal in JX1.
            if let Some(chan) = unaos_kernel::arch::bpmp_tegra::jb1b_ping(&geom) {
                // JB5 (attended bench): resolve the XUSB ids FIRST (a pure DTB read, no MMIO)
                // so the raw-handoff Falcon witness can run BEFORE any XUSB-affecting MRQ —
                // the whole point is to see the state UEFI handed over, untouched. A read-only
                // MRQ_PG GET_STATE guards the first MMIO touch (the JX1 gated-block rule); the
                // minimal BAR2 route is the one physically-unavoidable mutation (UEFI hands
                // FPCI over with BARs unprogrammed — see the JB5 block comment in xusb_tegra).
                let jb5_ids = unaos_kernel::arch::fdt_tegra::xusb_ids(
                    boot_info.dtb_addr,
                    boot_info.dtb_size,
                    mmu.ram_gib_mask,
                );
                if unaos_kernel::arch::xusb_tegra::JB5_PROBE {
                    match jb5_ids.as_ref() {
                        Some(ids) if unaos_kernel::arch::bpmp_tegra::jb5_pg_on(&chan, ids) => {
                            unaos_kernel::arch::xusb_tegra::jb5_bar2_route();
                            unaos_kernel::arch::xusb_tegra::jb5_witness("raw-handoff");
                            // JB6 probe: is CPUCTL=0xffffffff a stuck CSB page-select or a dead
                            // Falcon core? Read-only ARU/CSB sweep (only page-select writes).
                            unaos_kernel::arch::xusb_tegra::jb6_csb_sweep(); unaos_kernel::arch::xusb_tegra::jb11_fw_header_census("raw-handoff"); // JB11: the blob-free Falcon fw-header readout (BAR2 IOCTL_CFGTBL_READ) — always-on, its own bounded CNR wait, and an explicit all-ones = inherited-state-wall verdict. On the SAME line as the JB6 sweep so the wire-in adds ZERO source lines before any panic `Location` (the byte-identity convention documented at the main.rs tail) — non-tegra images are unmoved.
                            // JB7 (arc A): census the Falcon clocks' ACTUAL enabled state — jb1c
                            // only proves each MRQ_CLK ENABLE *acked*, not that the clock runs. A
                            // pure BPMP query on the untouched inherited handoff state; clock-gated
                            // vs reset-held. (The alternate FPCI/CFG CSB cross-read was tried on the
                            // first JB7 metal boot and is EL3-FATAL on the halted block — SError to
                            // BL31 — so it was removed; see the xusb_tegra JB7 note.)
                            unaos_kernel::arch::bpmp_tegra::jb7_clocks_query(&chan, ids);
                            // JB9-A probe point 1: FW liveness at raw handoff, CPUCTL-free
                            // (JB8: CPUCTL is a CSB priv-lock read, never a liveness witness).
                            unaos_kernel::arch::xusb_tegra::jb9_fw_alive("raw-handoff");
                            // JB9f: the inherit-run discriminator — MUST run here, before any
                            // UnaOS reset touches the controller: resume UEFI's own halted
                            // state (RS=1, no HCRST) and watch its inherited event ring for
                            // autonomous port-status-change posts.
                            unaos_kernel::arch::xusb_tegra::jb9f_inherit_run_probe();
                        }
                        Some(_) => serial_println!(
                            ":: tegra: JB5 — XUSB domain not ON at handoff; raw witness SKIPPED (JX1 rule) ::"
                        ),
                        None => {}
                    }
                }
                // JB0: fan next. The UEFI ExitBootServices teardown stopped the cooling fan
                // (it disabled the PWM3 clock + reset); restore it early so the SoC has
                // cooling for the rest of the boot. Cheapest teardown-restore (no power-gate),
                // rides the just-proven BPMP channel. Safety hygiene: a dead fan can't damage
                // the die (BL31/BPMP hardware thermal net), but this keeps it cool. (JB5 note:
                // the raw witness above deliberately precedes even this unrelated-PWM MRQ.)
                unaos_kernel::arch::bpmp_tegra::jb0_fan_on(&chan);
                match jb5_ids {
                    Some(ids) => {
                        xusb_alive =
                            unaos_kernel::arch::bpmp_tegra::jb1c_ungate_xusb(&chan, &ids);
                        // (JD4: the JB2c padctl re-power-up that ran here on pre-inherit boots was
                        // retired — on the JB9g/h inherit path the JB6 shim keeps UEFI from tearing
                        // the pads down, so they arrive live and the RMW was dead code.)
                    }
                    None => serial_println!(":: tegra: JB1c — no usb@3610000 ids in DTB; SKIP ::"),
                } #[cfg(feature = "jd1dc")] unaos_kernel::arch::display_tegra::jd1_dc_probe(&chan, boot_info.dtb_addr, boot_info.dtb_size, mmu.ram_gib_mask); #[cfg(feature = "ga10bprobe1")] unaos_kernel::arch::ga10b_probe::ga10bprobe1_run(&chan, boot_info.dtb_addr, boot_info.dtb_size, mmu.ram_gib_mask); // JD1-DC (UNAOS_JD1DC=1, default OFF) — the BPMP-guarded, READ-ONLY nvdisplay register probe; see the JD1-DC tail block in display_tegra.rs. THIS instruction is the only point in the boot where both of its ordering constraints hold: BPMP-FIRST (the `chan` this borrows was established by `jb1b_ping` ~60 lines up, and without it the MRQ_PG GET_STATE guard that earns the first read cannot be asked — which is why the probe could NOT stay at its old site inside `jd1_survey`), and JD1-FIRST (the scanout resolution, `map_fb_region`, `fbcon::init` and the `WRITER` seed are ~90 lines up, so panel + serial + shell are already live on a framebuffer resolved by a pure DTB RAM walk and a probe that goes wrong costs the experiment, not the boot). Last in the BPMP block so every other diagnostic has already reached the wire before the one read that could end it. Appended to this line, never a new one: knob-off it is cfg-erased and no panic `Location` below moves. GA10B-PROBE1 (UNAOS_GA10B_PROBE1=1, default OFF) — the first read-only GA10B iGPU probe rung: it borrows the SAME `chan` `jb1b_ping` established (BPMP-first: the rail gate that earns the first BAR0 read cannot be asked without it), reads the risk-ordered register list announce-first, and ENDS THE BOOT in PSCI SYSTEM_OFF (`power::shutdown`) per the cold-boot bench law — so it never returns and no later boot phase runs on a probe flight. Its own media by design (co-arming with other tegra knobs is not a supported flight). Appended to this line, never a new one: knob-off it is cfg-erased and no panic `Location` below moves.
            }
        }
        None => serial_println!(":: tegra: JB1b — geometry unresolved from DTB; SKIP ::"),
    }

    // 2. Boot banner: the same EL / CNTFRQ / MMU / DAIF snapshot `arch::boot_diagnostics` prints, read
    // straight from system registers (zero MMIO — cannot fault). Now the first REAL EL/CNTFRQ values
    // from Orin silicon (R4 crashed before this line); MMU reads `on` — our regime is live.
    let (el, cntfrq, sctlr, daif): (u64, u64, u64, u64);
    unsafe {
        let current_el: u64;
        core::arch::asm!("mrs {}, CurrentEL", out(reg) current_el, options(nomem, nostack, preserves_flags));
        el = (current_el >> 2) & 0b11;
        core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) cntfrq, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, DAIF", out(reg) daif, options(nomem, nostack, preserves_flags));
        if el == 2 {
            core::arch::asm!("mrs {}, SCTLR_EL2", out(reg) sctlr, options(nomem, nostack, preserves_flags));
        } else {
            core::arch::asm!("mrs {}, SCTLR_EL1", out(reg) sctlr, options(nomem, nostack, preserves_flags));
        }
    }
    serial_println!(":: UnaOS aarch64 kernel — Jetson Orin Nano (Tegra234), headless serial console ::");
    serial_println!(
        ":: AARCH64 boot diag: EL={}  CNTFRQ={} Hz  MMU={}  DAIF(DAIF)={:#06b} ::",
        el,
        cntfrq,
        if sctlr & 1 != 0 { "on" } else { "off" },
        (daif >> 6) & 0b1111,
    );
    // 3. JM4: bring up the Tegra234 GIC-600 + generic timer on the BOOT CORE so the heartbeat is
    //    interrupt-driven. We reuse the shared, EL2-aware interrupt path piece by piece rather than
    //    calling `arch::init()` (which would reprint its own "Core Hardware Init" + boot-diag banner).
    //    `gic::init` now walks the Tegra234 GICD/GICR bases (mapped by mmu_tegra's L1[0] device window);
    //    everything else — the CNTP timer at INTID 30, the shared IRQ stub — is identical to the
    //    QEMU-virt/Pi path. The exceptions.rs vector table (VBAR + the HCR_EL2.IMO routing) has been
    //    live since the JB1f install right after the MMU banner, so `enable_irq` below is the only
    //    arming step left here. SMP/other cores are a later arc — boot core only.
    serial_println!(":: tegra: JM4 — bringing up Tegra234 GIC-600 + generic timer (boot core) ::");
    unaos_kernel::arch::percpu::init(0);
    unaos_kernel::arch::gic::init();
    unaos_kernel::arch::timer::init();
    unaos_kernel::arch::timer::diagnose();
    unaos_kernel::arch::exceptions::enable_irq();
    unaos_kernel::arch::timer::verify_live();

    // 3c. Initialize the GLOBAL HEAP before the scheduler. CAPSTONE allocates (per-primitive waiter
    //     queues via `CAP_*.init()`, and each task's stack via `spawn`), so the heap MUST be live before
    //     `run_capstone_boot_core`. On every other path `kernel_main` calls `memory::init` at its line 4;
    //     the tegra path DIVERGES here (in `tegra_early_stop`) BEFORE `kernel_main` reaches that call, so
    //     without this the first CAPSTONE allocation would hit the empty `#[global_allocator]` (Heap::empty)
    //     → null → `handle_alloc_error` → panic — invisible in QEMU (tegra is never compiled into a
    //     regression) and a dead box on the Orin boot the arc's verdict depends on. mmu_tegra mapped RAM
    //     Normal-WB (identity), so the heap region `memory::init` carves is coherent both here (EL2) and,
    //     unchanged, after the JM6 drop (EL1) — the allocator state lives in RAM the reused L1 also maps.
    // (dtb fields are Copy; grabbed before `memory::init` consumes the &'static mut borrow.)
    let (dtb_addr, dtb_size) = (boot_info.dtb_addr, boot_info.dtb_size);
    unaos_kernel::arch::memory::init(boot_info);
    serial_println!(":: KERNEL HEAP ALLOCATED ::"); #[cfg(feature = "orindesk")] unaos_kernel::arch::display_tegra::orin_wm1(); // ORIN-WM1 — one wm window on the JD1 scanout (tail block)
    // XCARVE-2 temporal bracket: heap carved + span-B top published (`select_heap_region`). span-B now
    // covers the 0x26b900000 target, so this is the first bracket whose bisect can fire on the target.
    unaos_kernel::vugras::phase("post-heap-init");

    // ORIN-NET-1 (read-only PCIe/NIC census, `UNAOS_PCIEPROBE=1`): with the `pcieprobe` feature
    // armed, run the census HERE on the metal Orin — after JM4 (serial/heap live) and the mmu is up
    // (its RAM-GiB map gates the DTB deref), and BEFORE any of the JB2b xHCI work below (the census
    // is PCIe-only, independent of XUSB). Read-only: DTB census of every `pcie@` controller + a
    // guarded, poison-rejecting config-space liveness read for firmware-ENABLED controllers whose
    // aperture is already mapped (no new mapping, no fabric/config write). Compiled out knob-off, so
    // the default tegra image stays byte-identical to baseline. See arch_arm64.md §ORIN-NET-1.
    #[cfg(feature = "pcieprobe")]
    unaos_kernel::arch::pcie_probe::census(&unaos_kernel::arch::pcie_probe::PcieCtx {
        dtb_addr,
        dtb_size,
        ram_gib_mask: mmu.ram_gib_mask,
    });

    // ORIN-NET-2 (controller-0 link + device recon, `UNAOS_PCIE2=1`): with the `pcie2` feature armed,
    // run the recon HERE on the metal Orin — same preconditions as the NET-1 census (JM4 serial/heap
    // live, mmu up so the DTB deref and the `map_mmio_window` page-table patches are valid), before the
    // JB2b xHCI work. Reads controller-0 link state from the RP's DBI config space and, for a live link,
    // walks one level below; the ONLY writes are Device-nGnRE page-table mappings. Compiled out
    // knob-off, so the default tegra image stays byte-identical to baseline. See arch_arm64.md §ORIN-NET-2.
    //
    // ORIN-NET-3 (`UNAOS_PCIE3=1`, implies `pcie2`) extends this SAME `census2` call in place: the M1 PS
    // widen makes the ECAM reachable, and the metal `net3_*` path then performs the arc's three
    // fabric-write classes (appl LTSSM enable + BAR sizing, each logged before issue). See §ORIN-NET-3.
    #[cfg(feature = "pcie2")]
    unaos_kernel::arch::pcie_probe::census2(&unaos_kernel::arch::pcie_probe::PcieCtx {
        dtb_addr,
        dtb_size,
        ram_gib_mask: mmu.ram_gib_mask,
    });
    // XCARVE-2 temporal bracket: after the PCIe census/recon (NET-1/2/3). If the RAS fires between here
    // and post-heap-init, the store is in the PCIe bring-up window.
    unaos_kernel::vugras::phase("post-pcie");

    // ORIN-NET-4 (RTL8168/8111 GbE driver + smoltcp bind, `UNAOS_NET4=1`): with `net4` armed (implies
    // `pcie3`), run the driver bring-up HERE on the metal Orin — AFTER the NET-3 `census2` above has
    // widened the regime, enabled controller-0's LTSSM, and enumerated bus1:dev0:fn0. Claims the
    // Realtek device through the now-mapped ECAM, maps its register BAR, resets the MAC, brings up the
    // C+ RX/TX rings, reads the MAC, and binds a smoltcp phy::Device. Metal + tegra-gated (QEMU models
    // no Tegra234 RC). Compiled out knob-off => byte-identical to baseline. See arch_arm64.md §ORIN-NET-4.
    #[cfg(all(feature = "net4", feature = "tegra"))]
    unaos_kernel::arch::rtl8168_tegra::net4_bringup(dtb_addr, dtb_size, mmu.ram_gib_mask);
    // XCARVE-2 temporal bracket: after the NET-4 driver bring-up window (rings/DMA/MAC reset). If the RAS
    // fires between here and post-pcie, the store is in the NIC bring-up window. (Diagnostic line only —
    // does NOT touch the RTL8168 descriptor/ring code, which the sibling NET-4q arc owns.)
    unaos_kernel::vugras::phase("post-net");

    // ORIN-SDMMC-1 (`UNAOS_SDMMC=1`): the Tegra234 microSD-slot READ-ONLY recon on the metal Orin — the
    // installer line's first rung. Resolves the SDMMC controller from the live DTB, maps its window,
    // poison-honest CAPS probe, runs the SDHCI identification ladder (CID/CSD/capacity), and reads
    // sector 0 (CMD17) to classify MBR/GPT/FAT. READ-ONLY to the card by construction. Metal + tegra-gated
    // (QEMU models no Tegra234 SDMMC). Compiled out knob-off => byte-identical to baseline. See
    // arch_arm64.md §ORIN-SDMMC and scripts/orin-sdmmc1-bench.md.
    #[cfg(all(feature = "sdmmc", feature = "tegra"))]
    unaos_kernel::arch::sdmmc_tegra::sdmmc_census(dtb_addr, dtb_size, mmu.ram_gib_mask);

    // ORIN-SMP-7 (boot-state-context bisect) — the PRE-xHCI-takeover dispatch site. With `smpprobe`
    // armed to leg 25, the real 5-core wake fires HERE — after JM4 (GIC/timer/SMC/serial live) and
    // the heap, but BEFORE the JB2b `jb2b_attach` takeover / JB9i eviction below — so the woken cores
    // see a fabric the xHCI takeover has NOT yet mutated. Only leg 25 acts; every other armed value
    // is a silent no-op here and wakes at the post-takeover `smpprobe::run` site further down. The
    // dispatch position is the bisect's ONE variable (leg 24 = post-takeover, leg 25 = pre-takeover;
    // the pair brackets the takeover/eviction with no xusb_tegra.rs touch). Compiled out knob-off, so
    // the default tegra image stays byte-identical to baseline. See arch_arm64.md §ORIN-SMP-7.
    #[cfg(feature = "smpprobe")]
    unaos_kernel::arch::smpprobe::run_pre_xhci(&unaos_kernel::arch::smpprobe::ProbeCtx {
        dtb_addr,
        dtb_size,
        ram_gib_mask: mmu.ram_gib_mask,
    });

    // 3e. JB2b: platform-attach the shared xHCI driver at the XUSB block JB1c ungated, and pump
    //     the polled enumeration to a USB keyboard's armed interrupt-IN read — the Orin keyboard
    //     first-light arc. Runs HERE because it needs the heap (rings/contexts/buffers, step 3c)
    //     and the live EL2 timer (JM4 — the driver's bounded sync pumps `hlt()` between polls,
    //     and WFI needs the tick as its wake source; post-drop that would wedge, which is why the
    //     EL1 side below is a poll-only task). All bounded: a dead controller or wedged port is
    //     a few budgeted timeouts and an honest topology dump, then the JM6b chain proceeds
    //     unchanged. On success the keyboard keeps DMA-ing into identity-mapped RAM across the
    //     drop, and a pre-spawned task pumps it at EL1 (`xusb_tegra::kbd_pump_body`) — spawned
    //     onto the boot core's run queue NOW (pure RAM state; `poke_cpu` self-skips, so nothing
    //     is latched at the GIC to greet EL1), dispatched by `run_capstone_boot_core`'s drive
    //     loop cooperatively alongside (and after) the CAPSTONE tasks. No keyboard -> no task ->
    //     the JM6b/CAPSTONE flow is byte-identical to the JB1e verification boot.
    if xusb_alive {
        // JB3 (probe half): dump the NISO1 SMMU stream-match state BEFORE the attach, and the
        // global fault registers AFTER it — the JB2c verdict said the controller drives the
        // ports but cannot land one DMA write in RAM (ENABLE_SLOT watchdogs, 0 events), the
        // predicted SMMU drop. Bases + the XUSB stream id come off the LIVE firmware DTB
        // (verify-don't-assume); if the tree doesn't cooperate, fall back to the researched
        // tegra234 values (dual MMU-500 @0x0800_0000/0x0700_0000, SID 0x0e) and say so.
        // Read-only this boot — the fix writes are boot 2, chosen by what this dump says.
        let (jb3_bases, jb3_n, jb3_sid) = match unaos_kernel::arch::fdt_tegra::xusb_iommu(
            dtb_addr,
            dtb_size,
            mmu.ram_gib_mask,
        ) {
            Some(io) => (io.bases, io.n_bases, io.sid),
            None => {
                serial_println!(
                    ":: tegra: JB3 — DTB iommus unresolved; probing predicted bases ::"
                );
                ([0x0800_0000u64, 0x0700_0000u64], 2, 0x0e)
            }
        };
        // JB9h / JD3: the JB3 "fabric chain" (SMMU re-arm with USFCFG=1, MC SID rewrite, FPCI
        // full-enable, ARU restore, the locked-CSB Falcon restart) belonged to the halted-Falcon
        // revival world — dead on the inherit path (JB9f proved the inherited fabric passes the
        // FW's DMA as-is) and RETIRED in JD3. `jb9h_skip` now gates only the JB9b SID levers below;
        // the post-attach fault/MC-error diagnostics and the SMMUv3 census stay (read-only).
        let jb9h_skip = unaos_kernel::arch::xusb_tegra::JB9H_SKIP_CHAIN;
        if jb9h_skip {
            serial_println!(":: tegra: JB9h — inherit path (JB3 fabric chain retired; fabric untouched) ::");
        }
        // JB3 boot-6 (discrimination): the v2 pair is open + fault-free yet DMA still dies —
        // a second killer sits downstream. Census the DTB's smmu/iommu nodes (find any
        // SMMUv3 without touching unknown addresses), dump the v3's CR0/GBPA state if one
        // exists, and bracket the attach with MC error-log reads.
        let v3 = unaos_kernel::arch::fdt_tegra::smmu_census(dtb_addr, dtb_size, mmu.ram_gib_mask);
        if let Some(v3_base) = v3 {
            unaos_kernel::arch::smmu_tegra::jb3_v3_dump(v3_base);
        }
        unaos_kernel::arch::smmu_tegra::jb3_mc_errs("pre-attach");
        // (JD3: the JB3 MC-SID/FPCI/ARU/Falcon re-arm chain and the JB4 Falcon-revival levers —
        // both dead on the inherit path — were retired here. The two firmware-destroying levers
        // stay guarded by the compile-time asserts in xusb_tegra.rs.)
        let coherent = unaos_kernel::arch::fdt_tegra::xusb_dma_coherent(
            dtb_addr,
            dtb_size,
            mmu.ram_gib_mask,
        );
        // JB9: the SMMU bases + SID + the DTB-resolved padctl AO aperture ride into the attach
        // so the enable-slot-pending forensic captures (t+200ms / t+5s inside the pump window)
        // can dump the stream binding + the FW-side SID view live.
        let jb9_ao = unaos_kernel::arch::fdt_tegra::xusb_padctl_ao(
            dtb_addr,
            dtb_size,
            mmu.ram_gib_mask,
        );
        // (JD4: the JB9b SID levers — the AO IFRDMA_STREAMID retag + the SMMU accept-0x7f
        // fallback — were retired; both were dead behind the JB9h inherit gate, and JB9f proved
        // the inherited fabric passes the FW's DMA as-is. The forensics that found the SID
        // mismatch live in the git record at JB9/JB10.)
        // JB9e: the low-ring discriminator — answered (silent; high-address theory refuted).
        // MUST stay off on the JB9g inherit path: its xhci::init runs the HCRST that kills the
        // inherited firmware before the no-HCRST attach gets its chance.
        if !unaos_kernel::arch::xusb_tegra::JB9G_NO_HCRST {
            unaos_kernel::arch::xusb_tegra::jb9e_low_ring_probe();
        }
        let attached = unaos_kernel::arch::xusb_tegra::jb2b_attach(
            coherent,
            &jb3_bases[..jb3_n],
            jb3_sid,
            jb9_ao,
        )
        .is_some();
        unaos_kernel::arch::xusb_tegra::jb5_witness("post-attach");
        // JB9-A probe point 3: FW liveness after the enumeration attempt (did the enable-slot
        // watchdog rounds leave the firmware's service loop dead or still answering?).
        unaos_kernel::arch::xusb_tegra::jb9_fw_alive("post-enum-attempt");
        // The post-attach witness: after the enumeration window (and any ENABLE_SLOT
        // watchdogs), sGFSR/sGFSYNR say whether THIS block recorded the kills — and name the
        // faulting StreamID.
        unaos_kernel::arch::smmu_tegra::jb3_faults(&jb3_bases[..jb3_n]);
        unaos_kernel::arch::smmu_tegra::jb3_mc_errs("post-attach");
        if let Some(v3_base) = v3 {
            unaos_kernel::arch::smmu_tegra::jb3_v3_dump(v3_base);
        }
        if attached {
            // JD2: the pump grew a console. `jd2_console_pump` polls the xHCI exactly like the
            // JB2b `kbd_pump_body` did, but on the first keystroke it takes over the JD1 panel
            // with a `Screen`-backed `Console` and dispatches lines through the shared shell —
            // the first interactive UnaOS session on the Orin. Headless boots (no JD1 scanout)
            // delegate straight to `kbd_pump_body`, preserving the JB2b serial evidence lines.
            // VUGRAS (RAS localizer, `UNAOS_VUGRAS=1`): dump the candidate-PA table now — heap +
            // xHCI rings/contexts/buffers are live, and the enumerating port is flagged — so a decoded
            // RAS fault ADDR can be matched from the capture alone. No-op with the knob off.
            unaos_kernel::vugras::boot_witness();
            unaos_kernel::arch::sched::spawn("jd2-console", jd2_console_pump, 0, 0);
            serial_println!(":: tegra: JD2 — EL1 console pump task spawned (boot core) ::");
        }
        // XCARVE-2 temporal bracket: after the JB2b xHCI attach + enumeration window (the event-ring/ERST
        // move 425090fd lives here). If the RAS fires between here and post-net, the store is in the xHCI
        // bring-up window. Placed outside the `attached` guard so the bracket lands on every tegra boot.
        unaos_kernel::vugras::phase("post-xhci-attach");
    } else {
        serial_println!(":: tegra: JB2b — SKIPPED (XUSB not ungated/alive this boot) ::");
    }

    // ORIN-INSTALL-2 (`UNAOS_INSTALL_TARGET_SD=1`): the self-clone install runs HERE — AFTER the JB2b
    // pump window above enumerated the USB boot stick as a block device (`drivers::block::info()` is now
    // Some), so the installer can read the RUNNING system's real boot payload off the stick's ESP and
    // clone it onto the freshly-formatted microSD ESP. This is the DEFERRED half of the act INSTALL-1's
    // pre-JB2b census site could not complete (the stick was not yet a block device there → synthetic
    // marker). The read-only census at `sdmmc_census` above (line ~1396) stashed the card identity; this
    // consumes it. Earliest safe position: the stick is readable, the SDMMC MMIO the census mapped is
    // still live, and the core is still at EL2 (the JM6 drop is BELOW), so the SD path's bounded `hlt()`
    // waits still have the JM4 timer as their wake source. Metal + tegra-gated; compiled out knob-off =>
    // byte-identical to baseline. See arch_arm64.md §ORIN-INSTALL-2 and scripts/orin-sdmmc1-bench.md.
    #[cfg(all(feature = "install_target", feature = "tegra"))]
    unaos_kernel::arch::sdmmc_tegra::sdmmc_install_from_usb(); #[cfg(all(feature = "tegra", feature = "selfup"))] unaos_kernel::arch::selfup_tegra::selfup_service(); // ORIN-SELFUP: self-update service — same preconditions as ORIN-INSTALL-2, AFTER it so an install and an update on one boot see the media final; appended to THIS line for knob-off byte-identity (no line moves). See docs/dev/OS/10_INSTALL/orin-selfupdate.md.

    // ORIN-UNAFS-ROOT rung 4 (arc M4): probe-mount the microSD's NATIVE unafs volume on its OWN
    // block handle — `unafs::mount_on(BlockHandle::TegraSd)`, the consumer the SDSEAM arms in
    // fs/unafs.rs were written for. Position is load-bearing, three ways:
    //   * AFTER `sdmmc_census` (~line 2188): the census's `publish_block_backend` is what fills
    //     `TEGRA_SD_BLOCK_DEVICE`; before it, `tegra_sd_info()` is None and every read answers
    //     `NotReady` — mounting earlier could only witness its own prematurity.
    //   * AFTER `sdmmc_install_from_usb` (directly above): the installer REWRITES card content
    //     (never capacity). A mount taken between census and install would read the pre-install
    //     bytes and report a stale verdict on the very boot that changed the answer.
    //   * BEFORE the JM6 drop / CAPSTONE (below): still at EL2 with the JM4 timer live, so the
    //     polled CMD17 path's bounded waits behave exactly as they did for the census's own M3
    //     read; and the JD2 console task, though already spawned, is not dispatched until
    //     `run_capstone_boot_core`, so this witness lands on serial before any shell interaction.
    // PROBE, not an installation: `mount_on` returns a fresh `KernelUnaFS`, witnessed and DROPPED.
    // The shared `fs::unafs::MOUNT` cache (what the shell's `with_unafs` verbs use) stays bound to
    // `BlockHandle::Global` — the USB boot stick — untouched: rebinding it is fs-lane work and a
    // negotiation with the pi seat, not this rung. Two live mounts would be the K4 write-coherence
    // hazard unafs.rs names; dropping ours before proceeding, plus the block layer's unconditional
    // `write_block_tegra_sd` refusal (the card has NO writer outside `sdmmc_arm`), keeps this
    // read-only and momentary by construction. `program_source` is likewise undisturbed — its
    // `TegraSd` arm is `None` ("cannot be reached"), so the EL0 program volume stays the stick.
    // Compiled out without `sdmmc` => byte-identical to baseline. See docs/dev/OS/orin-unafs-root.md.
    #[cfg(feature = "sdmmc")]
    {
        use unaos_kernel::drivers::block;
        use unaos_kernel::fs::unafs;
        match block::tegra_sd_info() {
            // Publish absent: the census stopped before M3 (or refused num_blocks=0). This is the
            // expected line while the census SError is being fixed — the probe names the missing
            // precondition instead of a misleading NoStorage.
            None => serial_println!(
                ":: TEGRA-UNAFS: probe SKIPPED — no TegraSd block backend published this boot (census did not reach its publish) ::"
            ),
            Some(dev) => match unafs::mount_on(block::BlockHandle::TegraSd) {
                Ok(fs) => {
                    serial_println!(
                        ":: TEGRA-UNAFS: native unafs volume MOUNTED read-only on TegraSd — card {} sectors, {} committed root flip(s) on the volume ::",
                        dev.num_blocks,
                        fs.commit_stats().commits
                    );
                    // Dropped HERE: one mount at a time (K4), and the shared MOUNT stays Global.
                    drop(fs);
                }
                // The expected verdict until the installer has written a unafs partition: the card
                // is readable end to end (the partition scan ran off it), it just carries no volume.
                Err(unafs::MountError::NoVolume) => serial_println!(
                    ":: TEGRA-UNAFS: card readable ({} sectors) but NO partition carries a unafs superblock — installer has not written the native volume yet ::",
                    dev.num_blocks
                ),
                // Partition table unparseable — a raw/blank card reads this way; also expected pre-install.
                Err(unafs::MountError::Part(e)) => serial_println!(
                    ":: TEGRA-UNAFS: partition scan on TegraSd FAILED — {:?} (card {} sectors; raw/blank card reads this way pre-install) ::",
                    e,
                    dev.num_blocks
                ),
                // Everything else (NoStorage race, BadSectorSize, Fs, Busy): a real defect worth a capture.
                Err(e) => serial_println!(
                    ":: TEGRA-UNAFS: mount on TegraSd FAILED — {:?} (card {} sectors; unexpected — capture this boot) ::",
                    e,
                    dev.num_blocks
                ),
            },
        }
    }


    // 3d. JX1 RESULT (probe removed — metal-answered 2026-07-06, capture serial-orin-jx1.log): the
    //     Tegra234 XUSB host block @ 0x0361_0000 is NOT accessible after ExitBootServices. The
    //     probe's first read fired an SError (ESR 0xbe000011, EC=0x2F/ISS=0x11) fatal to EL3 —
    //     "Unhandled Exception in EL3" + BL31 crash dump — i.e. the CBB fabric aborts the access:
    //     UEFI tears its USB stack down at EBS and the XUSB partition is clock-gated/powered off
    //     (the JM5 CPU_ON class of wall; a gated Tegra block is an EL3-fatal touch, not an open-bus
    //     read, so no guarded-read pattern can probe it safely). DO NOT touch 0x0361_0000 (or other
    //     gated Tegra blocks) without first ungating via BPMP — the keyboard/mouse arc must bring up
    //     the tegra-bpmp IVC channel (HSP doorbell + shared memory, MRQ_CLK/MRQ_RESET to enable the
    //     XUSB partition, then the XUSB firmware question) BEFORE any xHCI register is readable.

    // ORIN-SMP-2 (JM5 `CPU_ON` firmware-wall INVESTIGATION): if `UNAOS_SMPPROBE` armed this image
    // (the `smpprobe` feature), run ONE serial-recorded probe experiment HERE — after JM4 (GIC/timer/
    // heap up, so SMC + AFFINITY_INFO + serial all work) and BEFORE the JM6 drop. Probe-only: the
    // query experiments (0/1/2/4) return and the boot proceeds to CAPSTONE unchanged; the CPU_ON
    // experiments (3/5) are the pre-registered power-fault boots (the box may RAS-fault and power off —
    // that is DATA, see the runbook `scripts/orin-smp2-bench.md`). Compiled out entirely when the
    // feature is off, so the default tegra image is byte-identical to baseline. (ORIN-SMP-7: this is
    // ALSO the POST-xHCI-takeover dispatch site — leg 24's control wake fires here, after the JB2b
    // takeover/JB9i eviction above; leg 25's pre-takeover complement fired at the early site. See
    // arch_arm64.md §ORIN-SMP-7.)
    #[cfg(feature = "smpprobe")]
    unaos_kernel::arch::smpprobe::run(&unaos_kernel::arch::smpprobe::ProbeCtx {
        dtb_addr,
        dtb_size,
        ram_gib_mask: mmu.ram_gib_mask,
    });

    // ORIN-SMP-3 (the real 6-core Orin bring-up): with the `tegrasmp` feature armed, kick off the
    // secondaries HERE — after JM4 (GIC/timer/heap/SMC all live) and while the BSP is still at EL2
    // (before the JM6 drop, so the woken cores replay the EL2 regime). Presence is sourced from the
    // DTB `/cpus` node ALONE (RIDER 1); the kick-off STOPs (single-core) if `/cpus` names nothing.
    // §ORIN-SMP-DEFAULT: `tegrasmp` is DEFAULT-ON for every tegra build (arroyo arms it for
    // `UNAOS_TEGRA=1` and `esp-jetson` alike, metal-proven across SMP-1..8) unless `UNAOS_NOTEGRASMP=1`
    // opts out, dropping call + enumerator => pre-flip baseline. Flip record: arch_arm64.md §ORIN-SMP-DEFAULT.
    #[cfg(feature = "tegrasmp")]
    unaos_kernel::arch::smp_virt::start_secondaries_tegra(dtb_addr, dtb_size, mmu.ram_gib_mask);

    // 4. JM6: drop the Orin BOOT CORE EL2 -> EL1 and run the scheduler + full M4 CAPSTONE at EL1 — the
    //    first time the scheduler runs on Orin silicon. This is the tegra analogue of the virt JC3 call
    //    site (`kernel_main`), and it becomes the tegra terminus: `run_capstone_boot_core` never returns.
    //
    //    STALE-COMMENT REPAIR (2026-08-25; this block previously said "Single-core, by design / JM5
    //    PARKED", which §ORIN-SMP-DEFAULT made false): secondaries ARE woken by default on tegra —
    //    `start_secondaries_tegra` runs above (`UNAOS_NOTEGRASMP=1` is the opt-out), CPU_ON succeeds on
    //    most boots, and the intermittent ORIN-SMP-3 park (~30%, H1: Device-typed instruction fetches
    //    pre-MMU) is instrumented by SMPMARK, not avoided by staying single-core. The APs stay at EL2;
    //    only the boot core drops. See arch_arm64.md §ORIN-SMP-DEFAULT.
    //
    //    JM4 above brought the timer up and PROVED IRQ delivery at EL2 (`verify_live`); the drop then
    //    disables the EL2 one-shot. The old "no IRQ may hit `__vec_irq` at EL1" rationale died at
    //    0a60e260 (runtime CurrentEL banking): IRQEL-RT proved an IRQ taken AT EL1 through this vector,
    //    and ORIN-BSPTICK (UNAOS_BSPTICK) runs a periodic EL1 tick across the terminus. Default CAPSTONE stays cooperative (`on_tick` only); ORIN-BSPRUN (UNAOS_BSPRUN, Candidate B arc 2 — implies bsptick) swaps the terminus to the preemptive `run_bsp_tegra(0)`: SCHED_ACTIVE + mark_online(0) + `run()`, no CAPSTONE spawn. See sched.rs tail §ORIN-BSPRUN.
    //
    //    JM7 (video): fbcon is deliberately NOT detached here (contrast JC3/virt, whose EL1 map omits
    //    the fb). mmu_tegra mapped the GOP GiBs into BOTH the live EL2 table and the EL1 twin, so the
    //    mirror keeps working across the drop — a connected monitor shows the pre-drop dump, the EL1
    //    landing line, and the CAPSTONE run live. Headless (fb addr=0): fbcon is inert, so keeping it
    //    attached is the same no-op the old detach was.
    serial_println!(
        ":: tegra: JM6 — dropping the Orin boot core EL2 -> EL1 for the scheduler + CAPSTONE ::"
    );
    // The EL1 regime gets mmu_tegra's EL1-PRECISE twin table (`mmu.ttbr0_el1`), NOT the live EL2 L1:
    // EL2 leaves carry AP[1] (RES1 at EL2), which the EL1&0 regime reads as "EL0-writable", and the VMSA
    // forces PXN=1 on any EL0-writable region — reusing the EL2 table made all RAM unexecutable at EL1
    // (the original JM6 dark hang; see boot_tegra.rs). The EL1 arm is dormant (SCTLR_EL1.M set while
    // still at EL2) until the eret, so EL1 never runs an instruction with its MMU off.
    //
    // Verify-don't-assume (the JM6 investigation plan): print the drop's actual inputs at EL2 first —
    // HCR_EL2 as JM4 left it (the drop rewrites it to RW-only), ID_AA64MMFR1_EL1.VH (bits [11:8]; VHE
    // present on A78AE, and E2H=1 handoff would redirect every *_EL1 msr above — must be non-VHE here),
    // the twin table's PA, and its two load-bearing descriptors read back from RAM: L1_EL1[0] (Device
    // window) and L1_EL1[code GiB] (must be AP[2:1]=0b00, PXN=0 — EL1-executable).
    {
        let (hcr, mmfr1, pc): (u64, u64, u64);
        unsafe {
            core::arch::asm!("mrs {}, HCR_EL2", out(reg) hcr, options(nomem, nostack, preserves_flags));
            core::arch::asm!("mrs {}, ID_AA64MMFR1_EL1", out(reg) mmfr1, options(nomem, nostack, preserves_flags));
            core::arch::asm!("adr {}, .", out(reg) pc, options(nomem, nostack, preserves_flags));
        }
        let code_gib = (pc >> 30) as usize;
        let l1 = mmu.ttbr0_el1 as *const u64;
        let (d0, dcode) = unsafe { (l1.read_volatile(), l1.add(code_gib).read_volatile()) };
        serial_println!(
            ":: tegra: JM6b pre-drop — HCR_EL2={:#x} MMFR1.VH={} TTBR0_EL1={:#x} L1_EL1[0]={:#x} L1_EL1[{}]={:#x} ::",
            hcr,
            (mmfr1 >> 8) & 0xf,
            mmu.ttbr0_el1,
            d0,
            code_gib,
            dcode,
        );
    }
    // Arm VBAR_EL1 at mmu_tegra's Part-C EL1 fault vector BEFORE the eret: under the fixed table the
    // vector is fetchable at EL1, so a residual landing fault prints an ESR/FAR/ELR syndrome line
    // instead of hanging dark. exceptions::install replaces it two lines below.
    unsafe { unaos_kernel::arch::mmu_tegra::arm_el1_fault_vector() };
    unsafe { unaos_kernel::arch::boot_tegra::drop_to_el1(mmu.ttbr0_el1) };
    // JD3: the drop just disabled the physical timer (CNTP_CTL=0) and the EL1 core has no interrupt
    // source, so mark the timer NOT-live — `verify_live` set it true at EL2 and that reading is now
    // stale. From here `arch::hlt()` busy-spins instead of a wake-less WFI-park, which is what lets
    // the post-drop panel shell's synchronous USB-MSC reads make progress: `ls`/`cat` ->
    // `block::read_block` -> the BOT pump, whose `crate::hlt()` yields now spin (bounded by the
    // pump's free-running-counter wall-clock deadline) rather than parking this timerless core.
    unaos_kernel::arch::timer::set_not_live();
    // Now at EL1 with the MMU live and DAIF masked. Print the landing proof (CurrentEL + the live
    // SCTLR_EL1) — the first EL1 serial line ever on Orin silicon — then re-seed this core's per-CPU
    // block (now TPIDR_EL1) and install the EL1 exception vectors (VBAR_EL1) — both pick the EL from
    // CurrentEL at runtime (JC3) — then run CAPSTONE cooperatively.
    {
        let (current_el, sctlr): (u64, u64);
        unsafe {
            core::arch::asm!("mrs {}, CurrentEL", out(reg) current_el, options(nomem, nostack, preserves_flags));
            core::arch::asm!("mrs {}, SCTLR_EL1", out(reg) sctlr, options(nomem, nostack, preserves_flags));
        }
        serial_println!(
            ":: tegra: JM6b — EL1 landing: CurrentEL={} SCTLR_EL1={:#x} ::",
            (current_el >> 2) & 0b11,
            sctlr,
        );
    }
    // EL0-EL1CORE — STAMP THE EL1 CORE, and do it here rather than one statement earlier or later.
    // `percpu::init` on the line above is the first instant at which BOTH facts the stamp records are
    // true: this core is at EL1 (the `drop_to_el1` two statements up returns only through its `eret`,
    // so reaching this line IS the drop having completed) and TPIDR_EL1 now resolves to this core's
    // block, so `cpu_index` is the index the SCHEDULER will dispatch under rather than a constant
    // asserted about it. `mark_el1_core` re-measures `CurrentEL` itself and stamps nothing if it is not
    // EL1, so the placement filter fails CLOSED — refusing every EL0 spawn — if this ever moves above
    // the drop. It must stay ABOVE `tegra_el0_start_maybe()` on the next line: that spawns the pinned
    // `el0-hello` task, and an unstamped mask would refuse it (see sched.rs `EL1_CORE_MASK`).
    unaos_kernel::arch::percpu::init(0); unaos_kernel::arch::sched::mark_el1_core();
    unaos_kernel::arch::exceptions::install();
    unaos_kernel::arch::timer::el1_oneshot_proof(); #[cfg(feature = "bsptick")] unaos_kernel::arch::timer::el1_bsptick_start(); tegra_el0_start_maybe(); #[cfg(feature = "tegradesk")] tegra_desk_arm(); #[cfg(feature = "orinconwin")] unaos_kernel::arch::display_tegra::orin_conwin(); #[cfg(feature = "orintenant")] unaos_kernel::arch::display_tegra::orin_tenant_arm(); #[cfg(feature = "orinladder")] unaos_kernel::arch::display_tegra::orin_ladder_arm(); #[cfg(feature = "orinfurn")] tegra_desk_furn(); #[cfg(feature = "orinrender")] tegra_render_arm(); tegra_rast_demo_maybe(); #[cfg(feature = "orinwdt")] unaos_kernel::arch::wdt_tegra::boot_ok_disarm(); #[cfg(not(feature = "bsprun"))] unaos_kernel::arch::sched::run_capstone_boot_core(0); #[cfg(feature = "bsprun")] unaos_kernel::arch::sched::run_bsp_tegra(0); // IRQEL-RT EL1 one-shot proof (first: one interrupt taken AT EL1 through the runtime-banked __vec_irq, then self-disarms — see timer.rs tail) + ORIN-BSPTICK periodic EL1 tick (no-op unless UNAOS_BSPTICK=1; arms right where the one-shot self-disarmed and leaves IRQs unmasked across the terminus — timer.rs tail) + ORINDESK RUNG 2 desktop seam (no-op unless UNAOS_TEGRADESK=1; DESKSEAM tail block) + ORIN-DESKFURN, the menu bar + SHARD menu (no-op unless UNAOS_ORINFURN=1; DESKFURN tail block — LAST of the rungs on purpose, so every earlier rung's probe still reads a panel with no bar on it and its captures stay comparable to boot7f/7g/7h; and still AHEAD of `boot_ok_disarm`, so the boot watchdog covers the composite it drives) + RAST-TEGRA demo (no-op unless UNAOS_RAST=1), all on the same line as the terminus so the wire-ins add ZERO source lines before any panic Location — the tegra knob-off byte-identity constraint (PI-V3D-1 bisect-proven). The terminus itself is knob-selected (ORIN-BSPRUN, Candidate B arc 2): default = the cooperative `run_capstone_boot_core(0)` drive loop, UNAOS_BSPRUN=1 = `run_bsp_tegra(0)` (sched.rs tail — SCHED_ACTIVE + mark_online(0) + the preemptive `run()` loop; requires/implies bsptick, arroyo folds it in). Helpers defined at file tail / sched.rs tail / timer.rs tail. Helpers defined at file tail / timer.rs tail.
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
        // GUI-CLICK-2: mark the screen app-owned across the (possibly long-running, full-screen)
        // command so the Pi USB pump leaves input in EVENT_QUEUE for the command's own pump_and_poll
        // (vug/pulse) instead of forwarding it into a GUI_CHANNEL that render_service — blocked HERE
        // inside dispatch_command — cannot drain. Cleared unconditionally on return (a took_screen
        // command has already restored the console by the time it returns).
        SCREEN_APP_ACTIVE.store(true, core::sync::atomic::Ordering::Relaxed);
        unaos_kernel::gui_watchdog::on_app_enter();
        let took_screen = unaos_kernel::shell::dispatch_command(&cmd, console, pal);
        unaos_kernel::gui_watchdog::on_app_exit();
        SCREEN_APP_ACTIVE.store(false, core::sync::atomic::Ordering::Relaxed);
        // TERM_RING (MIDDEN_CONVERGENCE §3, M2): THE DRAIN SITE. `dispatch_command` has returned, so
        // the render task owns the view again — the exclusive-drainer contract `termring::drain`
        // requires — and this is the first moment a record staged by a producer that is NOT this task
        // (an IRQ-masked context, a future second consumer's peer) can be moved into the scrollback.
        // Runs before the repaint below so anything drained is on screen this frame, and
        // unconditionally (a took_screen command has restored the console by the time it returns, and
        // its next repaint would otherwise be the first to show the backlog). `service` announces any
        // transport loss on the wire, on change only; both are no-ops on an empty, lossless ring, which
        // is what keeps the aarch64 path byte-identical.
        console.drain_output();
        unaos_kernel::termring::service();
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

/// JD11: the tegra command-output serial sink. `console.set_output_sink(jd2_out_sink)` routes every
/// `Console::println` line here, and it echoes the line on the serial UART framed to pair with the
/// per-keystroke `:: tegra: JD2 — KEY … ::` markers — so a single `awk '/:: tegra: JD2 —/'` over the
/// bench log reconstructs the whole interleaved session (keys typed + output produced). This is why
/// the marker keeps the `JD2` family and the `tegra:` token: it makes the Orin bench self-documenting
/// (the round-9 finding — panel output was uncapturable over serial). A plain `fn(&str)` with no
/// captured state; it only touches the serial UART (no re-entrancy into `Console`, no lock the caller
/// holds). Lives here (tegra-gated), not in the shared `console.rs`, so the marker string compiles
/// into the tegra kernel alone.
#[cfg(all(feature = "tegra", target_arch = "aarch64"))]
fn jd2_out_sink(line: &str) {
    serial_println!(":: tegra: JD2 — OUT | {} ::", line);
}

/// JD2 (tegra): the EL1 console pump — the Orin's interactive session, a cooperative kernel task
/// on the boot core's run queue (spawned pre-drop, dispatched by `run_capstone_boot_core` alongside
/// the CAPSTONE tasks). Supersedes the JB2b `kbd_pump_body` loop on panel-lit boots.
///
/// Phase 1 (boot log stays on the panel): poll the xHCI (`poll_events` ONLY — the `service_*` pumps
/// ride `hlt()`, and post-drop the EL2 timer is off, so a WFI would park this core forever; the
/// JB2b rule) and wait for the first HID keystroke. Phase 2 (the console owns the panel): detach
/// fbcon's serial mirror, build the double-buffered `Screen` over the JD1-inherited scanout
/// (`video::WRITER`, seeded in `tegra_early_stop`; the back buffer comes off the 48 MiB heap), and
/// feed every keystroke through the shared `handle_key` -> `shell::dispatch_command` (TABKEY: <TAB> is offered to the compositor's focus ring first, on `tegra_el0`), presenting
/// the damaged region after each. `Screen::flush` cleans the damage span to the Point of Coherency
/// (`dc cvac`) once per present — the DCE scans the carveout from DRAM and does not snoop.
///
/// Headless boot (no JD1 scanout -> WRITER never seeded): delegates to `kbd_pump_body`, so a
/// serial-only bench keeps the exact JB2b `KEY` evidence lines. Each keystroke is also echoed to
/// serial here — the attended-bench proof rides both channels. Busy-poll + `yield_now`, never
/// `sleep_ticks` (JC3 semantics: the drive loop drains no sleepers). Never returns.
#[cfg(all(feature = "tegra", target_arch = "aarch64"))]
fn jd2_console_pump(_arg: usize) {
    use unaos_kernel::pal::{Event, GneissPal};

    let front_fb = *unaos_kernel::video::WRITER.lock();
    if front_fb.info().width == 0 {
        unaos_kernel::arch::xusb_tegra::kbd_pump_body(0);
        // kbd_pump_body never returns; unreachable, but keep the flow explicit.
        return;
    }
    serial_println!(
        ":: tegra: JD2 — EL1 console pump live (boot log holds the panel; first key or ~8 s enters the shell) ::"
    );
    // XCARVE-2 temporal bracket: shell entry (boot-14/boot-20's crash path — the console pump is live with
    // no vug run). The last named bracket before the idle-sweep cadence takes over below.
    unaos_kernel::vugras::phase("shell-entry");

    // Phase 1 (JD4 screen-on-boot polish): pump the controller with the JD1 boot log visible, but
    // only until the FIRST KEYSTROKE or a ~8 s wall-clock deadline — whichever comes first — so a
    // panel-lit boot always ends at a visible shell prompt instead of waiting for a blind keystroke.
    // The bound rides CNTPCT (free-running, EL-independent — the JD3 timerless mechanism; the
    // post-drop EL1 core has no timer IRQ), and 8 s is past the CAPSTONE stragglers, so taking the
    // panel then cannot race a late fbcon paint. A keystroke keeps the JD2 behaviour byte-alike.
    let deadline_ticks: u64 = {
        let f: u64;
        unsafe {
            core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) f, options(nomem, nostack, preserves_flags));
        }
        (if f == 0 { 62_500_000 } else { f }).saturating_mul(8)
    };
    let cntpct = || -> u64 {
        let v: u64;
        unsafe {
            core::arch::asm!("mrs {}, CNTPCT_EL0", out(reg) v, options(nomem, nostack, preserves_flags));
        }
        v
    };
    // VUGRAS (RAS localizer): a ~250 ms writeback-sweep cadence for the shell-idle path (boot-14
    // crashed here with no vug run). `sweep_ticks` = CNTFRQ/4; the counter drives the alternating
    // A/B span sweep. No-op with the knob off. Shared by phase 1 and phase 2 below.
    let sweep_ticks: u64 = {
        let f: u64;
        unsafe {
            core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) f, options(nomem, nostack, preserves_flags));
        }
        (if f == 0 { 62_500_000 } else { f }) / 4
    };
    let mut last_sweep = cntpct();
    let mut sweep_tick: u64 = 0;

    let phase1_start = cntpct();
    let first_key: Option<u8> = loop {
        if let Ok(mut x) = unaos_kernel::drivers::xhci::claim() {
            x.poll_events();
        } #[cfg(feature = "orinrx")] unaos_kernel::arch::serial::serialrx::drain(); // SERIALRX (ORINRX) — THE UART RX DRAIN, the one statement this console path never had. Every pass polls UARTC (`tegra::read_byte`: LSR.DR + RBR, compiled into every jetson image and reachable through `arch::poll_input`) and pushes each byte as `Event::Key` onto the SAME queue the xHCI HID decoder feeds, so the `next_event` drain below sees a serial byte exactly as it sees a keystroke. Until this arc the only tegra callers of that poll were `pal::pump_and_poll` (vug, the selftest pager) — `next_event` is `pop_event` alone, no MMIO — so bytes typed at the wire never reached the shell. Four sites: this one, the phase-2 twin, `jd2_supstate_phase2` (without it the knob is inert on `supstate` images) and the headless `kbd_pump_body`. Same shape as `pump_and_poll`'s aarch64 arm in pal.rs. Diagnostics (`[serialrx] lsr=` one-shot, `[serialrx] rx=` census) live in the serial.rs tail mod. DEFAULT OFF — `BASE` is unverified on the board and a wrong BASE means phantom bytes into the shell. ⚠ LINE-NEUTRAL append: panic `Location` records embed line numbers and the knob-off jetson image byte-identity is this track's standing proof.
        match unaos_kernel::pal::next_event() {
            Some(Event::Key(c)) => break Some(c),
            _ => {
                if cntpct().wrapping_sub(phase1_start) >= deadline_ticks {
                    break None; // quiescent boot — take the panel and show the prompt
                }
                if cntpct().wrapping_sub(last_sweep) >= sweep_ticks {
                    last_sweep = cntpct();
                    unaos_kernel::vugras::idle_sweep(sweep_tick); #[cfg(feature = "rast")] unaos_kernel::arch::display_tegra::orin_rast_census(sweep_tick); #[cfg(feature = "orinrx")] unaos_kernel::arch::serial::serialrx::census(sweep_tick); // SERIALRX (ORINRX) — the `[serialrx] rx=` census on this EXISTING sweep cadence (every 4th tick = ~1 s, the `[orinrender] census` rate); no new timer. ⚠ LINE-NEUTRAL append. // ORIN-RASTGLASS: the `late` read-back, and THIS is the site that makes it mean anything — phase 1 is the interval between RAST returning and the console taking the panel, i.e. the only window in which the cube is supposed to be visible, and it is bounded at 8 s / 32 sweeps. The phase-2 twin below keeps sampling afterwards, but a repaint observed THERE is the console doing its job (RAST-SUPERSEDED-BY-CONSOLE), not the defect. ⚠ LINE-NEUTRAL append.
                    sweep_tick += 1;
                }
                unaos_kernel::arch::sched::yield_now();
            }
        }
    }; #[cfg(feature = "supstate")] jd2_supstate_phase2(front_fb, first_key, sweep_ticks, last_sweep, sweep_tick); // ORIN-SUPSTATE (Candidate A arc 1) — knob-on, phase 2 runs with the surface LIFTED into `display_tegra::supstate`'s module-owned handle (never returns; the legacy phase 2 below is then dead code, kept verbatim as the knob-off path). ⚠ LINE-NEUTRAL append — knob-off this statement is `#[cfg]`-erased and the file's line numbering is untouched (panic `Location` byte-identity, the orinclick rule).

    // Phase 2: the console owns the panel. Detach the fbcon serial mirror FIRST so a CAPSTONE
    // straggler line can't paint over the console frame (serial output is unaffected).
    if !tegra_conwin_live() { unaos_kernel::video::fbcon::detach(); } #[cfg(feature = "rast")] unaos_kernel::arch::display_tegra::orin_rast_console_owns(); // ORIN-RASTGLASS: the phase-2 boundary latch — from here the console owns the glass, so a cube repainted away is the design, not a defect, and the census says RAST-SUPERSEDED-BY-CONSOLE instead of RAST-PAINTED-OVERWRITTEN. Placed on the FIRST statement of phase 2 and outside the conwin guard: the console takes the panel on both routes, only the detach differs. ⚠ LINE-NEUTRAL append. // ORIN-CONWIN rung 4 — the detach is GUARDED BY THE ROUTE, exactly as the Pi's GUI handoff guards it (`main.rs`'s CONSWIN-PI line). `tegra_conwin_live()` answers true only when `fbcon::console_is_routed()` does, and a routed console does not write the panel — `FbCon::draw_fb` hands back `win_fb`, kernel RAM no scan-out reads — so the ONE thing this detach exists to guarantee (exactly one writer on the panel while the JD2 Screen console owns it) is already true, and skipping it leaves the console window LIVE instead of freezing it with the boot log. Knob-off it is `#[inline(always)] false`, so this folds to the bare `detach();` it has always been. ⚠ FOLDED IN PLACE, never added lines: panic `Location` records embed line numbers and the knob-off jetson image's byte-identity is this track's standing proof.
    let mut screen = unaos_kernel::video::Screen::new(front_fb);
    // VUGRAS: the Screen back buffer PA is only known now — add it to the candidate table.
    unaos_kernel::vugras::note_screen(&screen);
    let mut pal = unaos_kernel::pal::TargetPal::new(&mut screen);
    let mut console = unaos_kernel::console::Console::new();
    // JD11: mirror every command-output line to serial so an attended Orin bench captures a durable,
    // mbench-able transcript (the panel has no scrollback; before JD11 only keystrokes echoed to
    // serial). Installed before the banner so the shell-entry lines head the transcript too.
    console.set_output_sink(jd2_out_sink);
    console.println("UnaOS — Jetson Orin Nano (Tegra234)");
    console.println("JD2: interactive shell on the inherited scanout. Type 'help'.");
    console.draw(&mut pal);
    pal.render();
    match first_key {
        Some(c) => {
            serial_println!( // ORINPATH — `path=` names WHICH phase-2 printed this, and until this arc nothing did. `jd2_supstate_phase2` carries a verbatim copy of these two literals; on a `supstate` build both copies pass `#[cfg]`, so neither a serial capture nor a grep of the artifact could attribute the line to a site. See the twin's comment for the MEASURED shape of that ambiguity at 0ed6fee2. Token deliberately longer than 8 bytes — a shorter mark can be LLVM-immediate-encoded and never appear in `.rodata`, defeating the artifact grep. Placed AFTER "console OWNS the panel" so `scripts/specs/jetson-sync1.spec` / `jetson-jd5.spec`'s `REQUIRE JD4.*console OWNS the panel` still matches.
                ":: tegra: JD2 — console OWNS the panel (Screen back buffer live); path=jd2-console-pump; first key {:#04x} ::",
                c
            );
            // The wake-up keystroke is a real keystroke: feed it through, don't swallow it.
            handle_key(c, &mut console, &mut pal);
            pal.render();
        }
        None => serial_println!(
            ":: tegra: JD4 — console OWNS the panel (Screen back buffer live); path=jd2-console-pump; screen-on-boot (no key, ~8 s) ::"
        ),
    }

    // JD20 (ORIN-POINTER): the composite pointer behind the HS hub already streams reports — the
    // shared xHCI decoder pushes Mouse/MouseAbsolute/Button onto the PAL queue (built for the x86
    // midden GUI; nothing consumed them on the Orin until now). This loop is the tegra-side pump:
    // it drains those events alongside keys and composites the SHARED `pal::cursor` sprite onto the
    // console's Screen back buffer via the R22 save-under machinery (stash the pixels under the
    // sprite, restore before any repaint), so the arrow never corrupts console text and survives
    // console redraws. The cursor self-gates on `visible()` (starts hidden, auto-hides ~1.5 s after
    // the last report), so a keyboard-only boot is byte-identical — no knob needed.
    use unaos_kernel::pal::cursor;
    let mut announced = false;
    loop {
        if let Ok(mut x) = unaos_kernel::drivers::xhci::claim() {
            x.poll_events();
        } #[cfg(feature = "orinrx")] unaos_kernel::arch::serial::serialrx::drain(); // SERIALRX (ORINRX) — phase-2 twin of the phase-1 drain above (see that line). ⚠ LINE-NEUTRAL append.
        let cursor_was_visible = cursor::visible();
        let mut needs_render = false;
        // A key repaints the console into the back buffer; when that happens the cursor was erased
        // first (restore, below) and must be re-composited on top after the drain.
        let mut key_repainted = false;
        // ORIN-POINTER-FIX (regression: keyboard went dead at the panel whenever a pointer was
        // present). An absolute tablet reports at every poll interval, so it streams MouseAbsolute
        // into the shared 64-deep event queue continuously — not only on motion. The first cut did
        // a full save-under `restore`+`draw_over`+`pal.render()` PER pointer event; under that flood
        // each frame turned into dozens of back-buffer read-backs + a DC-CVAC scanout flush, so the
        // pump drained the queue slower than the tablet filled it. `EventQueue::push` DROPS silently
        // when full (pal.rs), so the interleaved `Event::Key` pushes were the casualty — keys never
        // reached the drain. Pre-JD20 the pump popped-and-ignored the same flood for free and keys
        // got through. Fix: COALESCE all pointer motion in the frame to a single cursor update
        // (relative deltas accumulate; absolute takes the last position), so per-frame cursor work
        // is O(1) regardless of report rate and the drain always keeps pace with the keyboard. Keys
        // are handled inline and unconditionally, so they flow with zero pointer devices too.
        let mut pending_rel: Option<(i32, i32)> = None;
        let mut pending_abs: Option<(i32, i32)> = None;
        while let Some(ev) = unaos_kernel::pal::next_event() {
            match ev {
                Event::Key(c) => {
                    // Erase the cursor BEFORE the console repaints, so the save-under stash never
                    // captures cursor pixels and a later restore can't paint stale glyphs back.
                    cursor::restore(&mut pal);
                    // Serial echo: the bench evidence line (panel + serial must agree).
                    if (32..=126).contains(&c) {
                        serial_println!(":: tegra: JD2 — KEY '{}' ::", c as char);
                    } else {
                        serial_println!(":: tegra: JD2 — KEY {:#04x} ::", c);
                    }
                    needs_render = true;
                    key_repainted = true; #[cfg(feature = "tegra_el0")] if unaos_kernel::arch::aarch64::syscall::wc_shell_focus_key(ev) { continue; } // TABKEY — THE MISSING SHELL HALF OF THE FOCUS RING. `wc_focus_key` has always been reachable on tegra through the IN-RING door (`user_input_enqueue` asks it at syscall.rs's router seam, and `orininput`'s `oi_pump` drives that seam), but the SHELL door — the one that fires while `USER_INPUT_ACTIVE == 0`, which is this board's normal state — had NO CALLER AT ALL, because `main.rs`'s two `wc_shell_focus_key` sites both live inside `pump_usb_into_gui`, `cfg(all(aarch64, baremetal))`, and `baremetal` implies `pi` while `pi` + `tegra` is a hard `compile_error!`. So on the Orin <TAB> fell straight through to `handle_key`, which has no byte-9 arm, and was silently dropped. MEASURED on metal (`~/unaos-bench/capture/line-acm0/`): `orin.log` carries 5x `:: tegra: JD2 — KEY 0x09 ::` at :13071-13080 and ZERO `[wc-c] focus tab-cycle`; the Pi control `pi.log`, same tool and directory, has 249. The serial echo above is deliberately left AHEAD of this call so the pairing stays scoreable from a capture — a fixed board reads `KEY 0x09` immediately followed by `[wc-c] focus tab-cycle`, a regressed one reads `KEY 0x09` alone, which is exactly what the Orin capture reads today. `wc_shell_focus_key` and not `wc_focus_key`, because this pump IS the Orin's shell drain: it returns false the moment an EL0 program holds focus, leaving the in-ring case to the router seam that already owns it — one body, two doors, one witness, which is the Pi's arrangement verbatim. `continue` where the Pi's bare-shell drain `break`s: that loop's destination is fixed at `gui_send`, so a moved focus invalidates the rest of its drain, whereas this loop's destination is `handle_key` on the backdrop console, which this pump feeds REGARDLESS of focus anyway (`xusb_tegra.rs`'s defect-2 note, out of this arc's scope) — so breaking would only re-deliver the same events to the same place one pass later. Gated `tegra_el0` because `arch/aarch64/mod.rs` gates `pub mod syscall;` on `aarch64_el0`, which a bare `tegra` image does not carry; the knob-off tegra media is therefore byte-identical, which is why this is FOLDED onto the statement above and given no line of its own — panic `Location` records embed line numbers and a line added anywhere in this file breaks that proof.
                    if handle_key(c, &mut console, &mut pal) {
                        // A command took the whole screen (e.g. `gneiss`): stop draining this frame
                        // so a queued keystroke can't paint the console back over it (the shared
                        // drain-loop rule from the x86 GUI path).
                        break;
                    }
                }
                Event::Mouse { x, y } => {
                    // Coalesce: accumulate the relative delta; the cursor moves once, after drain.
                    let (ax, ay) = pending_rel.unwrap_or((0, 0));
                    pending_rel = Some((ax + x, ay + y));
                    if !announced {
                        serial_println!(
                            ":: tegra: JD20 — pointer live (relative mouse, cursor on scanout) ::"
                        );
                        announced = true;
                    }
                }
                Event::MouseAbsolute { x, y } => {
                    // Coalesce: last absolute position in the frame wins (raw HID 0..=32767).
                    pending_abs = Some((x, y));
                    if !announced {
                        serial_println!(
                            ":: tegra: JD20 — pointer live (absolute tablet, cursor on scanout) ::"
                        );
                        announced = true;
                    }
                }
                Event::Button(mask) => {
                    // JD20's raw line: an event reached the PUMP. ORIN-CLICK (rung 3) then hands the same edge to the window layer — a separate claim, kept on a separate line, so a capture can tell a decoder fault from a routing fault. ⚠ LINE-NEUTRAL: the routing call is APPENDED to the JD20 statement, never given a line of its own — panic `Location` records embed line numbers, so a line added anywhere in this file breaks the knob-off byte-identity proof.
                    serial_println!(":: tegra: JD20 — pointer BUTTON {:#04x} (down) ::", mask); #[cfg(feature = "orinclick")] unaos_kernel::arch::display_tegra::orin_click(mask); // ORIN-CLICK rung 3 — route the edge into `wc_click_route`; witness `[orinclick] edge=...`
                }
                _ => {}
            }
        }
        // One cursor composite for the whole frame's pointer activity — bounded work no matter how
        // many reports arrived. Any key handled above already erased the cursor and repainted the
        // console; drawing over the fresh back buffer here re-composites the arrow on top.
        if pending_rel.is_some() || pending_abs.is_some() {
            cursor::restore(&mut pal);
            if let Some((dx, dy)) = pending_rel {
                cursor::move_rel(dx, dy, pal.width() as i32, pal.height() as i32);
            }
            if let Some((ax, ay)) = pending_abs {
                // Absolute tablet coords are raw HID 0..=32767; `set_abs` scales to the panel.
                cursor::set_abs(ax, ay, pal.width() as i32, pal.height() as i32);
            }
            cursor::draw_over(&mut pal);
            needs_render = true;
        } else if key_repainted && cursor::visible() {
            // A key repaint erased the cursor and redrew the console; put the arrow back on top.
            cursor::draw_over(&mut pal);
        }
        // Auto-hide swept the cursor off this frame: erase it once so no arrow is left parked.
        if cursor_was_visible && !cursor::visible() {
            cursor::restore(&mut pal);
            needs_render = true;
        }
        if needs_render {
            pal.render();
        }
        // VUGRAS: the shell-idle writeback sweep on the ~250 ms cadence (boot-14's crash path — the
        // console pump was live at shell entry with no vug run). No-op with the knob off.
        if cntpct().wrapping_sub(last_sweep) >= sweep_ticks {
            last_sweep = cntpct();
            unaos_kernel::vugras::idle_sweep(sweep_tick); #[cfg(feature = "orinclick")] unaos_kernel::arch::display_tegra::orin_click_census(sweep_tick); #[cfg(feature = "orintenant")] unaos_kernel::arch::display_tegra::orin_tenant_census(sweep_tick); #[cfg(feature = "orinladder")] unaos_kernel::arch::display_tegra::orin_ladder_census(sweep_tick); #[cfg(feature = "rast")] unaos_kernel::arch::display_tegra::orin_rast_census(sweep_tick); #[cfg(feature = "holocron")] unaos_kernel::video::prtscr::service(); #[cfg(feature = "orinrx")] unaos_kernel::arch::serial::serialrx::census(sweep_tick); // ORIN-RASTGLASS: the `late` half of the cube read-back — combined with the latched `post` it separates painted-and-survived from painted-and-overwritten from never-painted, which the RAST path could not say at all. `rast` alone is the gate (it does NOT imply tegra), and this fn is already `cfg(all(tegra, aarch64))`, so the conjunction is exact. ⚠ LINE-NEUTRAL append — see the Button arm above. // ORIN-CLICK rung 3 — the ARM line, then the ~10 s click census. Emitted FROM THIS LOOP on purpose: it is the routing task's own liveness, so `[orinclick] census` stopping is a dead pump and `btn=0` is "nobody clicked", not "routing failed". ⚠ LINE-NEUTRAL append — see the Button arm above. // PRTSCR-ORIN — the Print Screen key's SERVICE half on the Orin. The xHCI decoder this loop drives (`claim` -> `poll_events` -> `handle_event_trb`, drivers/xhci) arms `prtscr::request()` on the HID 0x46 press edge on every arch, but the three `prtscr::service()` sites in this file are x86/virt-only (the `usbdebug` loop, the `kernel_main` tail below `tegra_early_stop`'s divergence, `x86_usb_pump`), so on tegra the flag was armed and never serviced. THIS task is the only pump every jetson image with a keyboard runs (the `orinrender` pass is knob-gated and drains no events), and the sweep is a point where no xHCI claim is held — `capture()` takes the xHCI loan for the FAT write to the boot stick in the global slot. `prtscr` has NO code dependency on `holocron` (`video/mod.rs` declares it unconditionally; `capture` drives `fs::fat` alone): the gate is the repo's arming knob for "this boot may WRITE its boot medium" (`UNAOS_HOLOCRON=1`, deliberately not stripped by `arm_features`), so the knob-off jetson image stays byte-identical. Idle cost: one relaxed load per ~250 ms sweep. ⚠ LINE-NEUTRAL append — see the Button arm above. // SERIALRX (ORINRX) — the `[serialrx] rx=` census, phase-2 twin of the phase-1 sweep site. ⚠ LINE-NEUTRAL append. // ORIN-RASTGLASS: the `late` half of the cube read-back — combined with the latched `post` it separates painted-and-survived from painted-and-overwritten from never-painted, which the RAST path could not say at all. `rast` alone is the gate (it does NOT imply tegra), and this fn is already `cfg(all(tegra, aarch64))`, so the conjunction is exact. ⚠ LINE-NEUTRAL append — see the Button arm above. // ORIN-CLICK rung 3 — the ARM line, then the ~10 s click census. Emitted FROM THIS LOOP on purpose: it is the routing task's own liveness, so `[orinclick] census` stopping is a dead pump and `btn=0` is "nobody clicked", not "routing failed". ⚠ LINE-NEUTRAL append — see the Button arm above.
            sweep_tick += 1;
        }
        unaos_kernel::arch::sched::yield_now();
    }
}

/// M5b: the keyboard-event channel from the input service to the render service (bare-metal aarch64).
/// The input thread `send`s Key events; the render thread `recv`s them — a cross-core handoff (the two
/// run on different APs), dogfooding the M4 `Channel`. Capacity 64 matches the old event ring; a full
/// channel applies backpressure to the input thread rather than dropping keystrokes.
///
/// SERIAL-FOCUS — "backpressure to the input thread" is the right policy for a HID producer that can
/// be made to wait, and it was the wrong one for the SERIAL CONSOLE, whose consumer parks for the
/// whole life of a foreground command. Serial no longer rides this channel as payload at all; see
/// `serial_to_shell`. The capacity is unchanged and the HID path is untouched.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static GUI_CHANNEL: unaos_kernel::arch::sched::Channel<unaos_kernel::pal::Event> =
    unaos_kernel::arch::sched::Channel::new(GUI_CHANNEL_CAP as usize);

/// SERIAL-FOCUS — `GUI_CHANNEL`'s capacity, named so the wake-headroom test in `serial_to_shell` is
/// derived from the channel rather than from a repeated literal that a later resize would desync.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
const GUI_CHANNEL_CAP: u64 = 64;

/// SCHED-X86: the x86 twin of `GUI_CHANNEL` — the input service `send`s, the render service `recv`s,
/// across two different application processors. Same capacity (64) and the same backpressure
/// contract: a full channel blocks the producer rather than dropping a keystroke. Separate from the
/// aarch64 static (rather than one ungated channel) because the two arches' handoffs are gated on
/// different feature sets and neither build should carry the other's 64-slot `VecDeque`.
#[cfg(target_arch = "x86_64")]
static GUI_CHANNEL_X86: unaos_kernel::arch::sched::Channel<unaos_kernel::pal::Event> =
    unaos_kernel::arch::sched::Channel::new(64);

/// SCHED-X86 (depth witness): events forwarded INTO / taken OUT OF `GUI_CHANNEL_X86`. `sent - recv`
/// is the live queue depth, which is the one number that distinguishes "the render task is keeping
/// up" from "the render task is wedged and the input task is about to block in `send`". Both sites
/// live in this file, so the pair cannot drift.
#[cfg(target_arch = "x86_64")]
static GUI_SENT_X86: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(target_arch = "x86_64")]
static GUI_RECV_X86: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// SCHED-X86: ms() of the last `[schedx86] depth` line, rate-limiting it to once every ~5 s. The
/// render task passes at least four times a second (the pulse), so this is a real clock gate and not
/// a pass counter standing in for one.
#[cfg(target_arch = "x86_64")]
static GUI_DEPTH_LAST_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// SCHED-X86: cadence of the render task's periodic wake, posted as an `Event::Timer` by the input
/// service. The render loop BLOCKS on `recv`, so without a pulse two things that are not driven by
/// input would never happen: the CURSOR-HIDE auto-hide erase (which fires ~1.5 s after the last
/// pointer report) and the `instgui` disk rescan. The Pi solves this with a dedicated `status-tick`
/// task; here it rides the input service's existing nap — a wall-clock compare per pass, no extra
/// task, no extra timer. 250 ms keeps the auto-hide crisp at four channel sends per second.
#[cfg(target_arch = "x86_64")]
const X86_GUI_PULSE_MS: u64 = 250;

/// INPUT-UNGATE: OFFER one event to `GUI_CHANNEL_X86` and bump the sent counter — the single choke
/// point, so `GUI_SENT_X86` can never disagree with the real send count.
///
/// **This replaced a blocking `gui_send_x86`, and the replacement is the arc.** The old choke point
/// called `Channel::send`, which parks the caller while the channel is full. That is correct
/// backpressure against a consumer that is merely SLOW and catastrophic against one that is DEAD,
/// and boot 16 produced the dead one: the render task's core parked inside a non-returning MMIO
/// store, so the channel's only consumer stopped existing, the 64 slots filled, and
/// `x86_input_service` parked in `send` for the remaining 497 seconds. Because that task is also the
/// only drain of `pal::EVENT_QUEUE`, the whole input path died behind the compositor.
///
/// The refusal is handed back rather than swallowed (`Err(ev)`), because WHAT TO DO ABOUT IT IS A
/// QUESTION ABOUT THE EVENT, not about the channel, and only the caller can answer it — see the
/// three-class policy in [`x86_input_service`]. `GUI_SENT_X86` is charged only on the accepted path,
/// so `sent - recv` keeps reading as live occupancy exactly as before.
#[cfg(target_arch = "x86_64")]
fn gui_try_send_x86(ev: unaos_kernel::pal::Event) -> Result<(), unaos_kernel::pal::Event> {
    match GUI_CHANNEL_X86.try_send(ev) {
        Ok(()) => {
            GUI_SENT_X86.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            Ok(())
        }
        Err(back) => {
            unaos_kernel::deadman::note_gui_declined();
            Err(back)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// WCSER-REHOME — A CORE THE STEAL DECLARED DEAD LOSES ITS SINGLETON ROLES; THEY DO NOT DIE WITH IT.
// ─────────────────────────────────────────────────────────────────────────────────────────────
//
// WCSER-STEAL closed half of a defect and named the other half as an accepted trade: *"the parked
// core is not recovered, its window is not recovered."* Boot 16 showed the trade was mispriced,
// because the list of what dies with the core was longer than the window. The render task lives on
// that core, and the render task is the ONLY consumer of `GUI_CHANNEL_X86`. When it died the channel
// acquired no consumer for the remaining 497 seconds, `x86_input_service` filled all 64 slots and
// parked in `Channel::send`, and — because that task is also the sole drain of `pal::EVENT_QUEUE` —
// the machine's entire input path went with it. `[deadman] hq=25`, pinned, unmoving, for seven
// minutes, while the desktop repainted at 66 composite passes a second on a stolen gate.
//
// So the steal is only half a recovery. It re-homes the PANEL. Everything else pinned to the dead
// core is re-homed here, on the principle stated generally because the general statement is the
// fix: **a core declared dead by the steal has its singleton roles reassigned, and any role that is
// NOT reassigned is named explicitly as a known-abandoned trade.** For this kernel that list is:
//
//   RE-HOMED — the render service (`x86_render_service`): the `GUI_CHANNEL_X86` drain, the event
//              dispatch and the panel present. Re-homed because its death is unrecoverable from the
//              producer side at any price — a non-parking producer facing a channel with no consumer
//              coalesces and holds forever, which is a quieter death, not a lesser one.
//   ABANDONED — the dead core itself, and the ring-3 window whose present was in flight when it
//              parked. Peter has accepted both; they are one core and one window, and nothing in
//              software un-parks a core stopped on an instruction. Named here rather than left to be
//              rediscovered.
//   NOT AT RISK — `x86_usb_pump` and `x86_input_service` share the SERVICE core, not the render
//              core, so the steal has never cost them anything. If a future steal names the service
//              core the same census must be run for them, and this comment is the place it is owed.
//
// ── THE CORPSE'S LOCKS — the hazard that could have made this fix worse than the defect ──────────
//
// `arch::x86_64::sched::Mutex` is `{ sem: Semaphore, data: UnsafeCell<T> }`: a bare semaphore permit
// with NO OWNER RECORD, and nothing in task teardown walks a dying task's held locks (`reap_killed`
// handles an explicit `kill`; a core parked on an instruction is never reaped at all). So **a task
// that dies mid-chain holds every lock it held, forever.** A replacement that then takes one of them
// parks forever too — and that is a SECOND freeze, quieter than the first, with nothing on the wire.
// It would convert "input dead, visible" into "input dead and the recovery also dead, invisible".
//
// So the question is not rhetorical and it is answered before the spawn, not after. What can the
// corpse hold at the instruction it stopped on — `stage_window`'s phase-33 row blit?
//
//   * `COMP_GATE` — yes, and that is the whole reason WCSER-STEAL exists. Stolen, and the release
//     side is identity-checked so a revenant cannot double-release it.
//   * `STAGE[stage_pool_index()]` — yes, but `stage_pool_index()` is `meter_current_cpu()`, so the
//     entry is PER-CORE. The replacement runs on a different core and composes into a different
//     entry. Structurally disjoint, not merely usually-free.
//   * `TABLE` (the window table) — NO. `composite_inner` takes it in a scoped block that produces the
//     row snapshot and the `BlitGuard` registration, and the guard is dropped at the end of that
//     block, well before the draw loop the corpse is stuck in. Boot 16 is the independent
//     confirmation: ~66 composite passes per second completed for the 510 s AFTER the steal, and
//     every one of them takes `TABLE`. If the corpse held it, none would have.
//   * `WRITER` — NO. `let fb = *super::WRITER.lock()` copies the handle and drops the guard on the
//     same line; nothing holds it across a blit.
//   * The heap lock — NO. The phase-33 row loop allocates nothing.
//
// That is why the rescue instance is safe to start, and it is also why it does NOT re-mint the shell
// window: `open_shell_window` allocates and takes the table, which is fine, but the row it would add
// duplicates one the corpse's surface still backs. Skipping it is the narrower action.
//
// KNOWN RESIDUAL, named rather than discovered later: the corpse's `BlitGuard` is never dropped, so
// its `BLIT_ACTIVE` registration stands for the rest of the boot. Nothing here waits on that count
// reaching zero — a raised `DRAIN_PENDING` makes a composite pass SKIP, not block — so it costs a
// leaked count and not a hang. It predates this arc (it is a property of the steal, not of the
// re-home) and boot 16 ran 510 s with it standing.

/// WCSER-REHOME — cores the steal has declared dead this boot, one bit each. Consulted when picking
/// a rescue core so a re-home can never land on a corpse (which would silently be a no-op forever).
#[cfg(target_arch = "x86_64")]
static DEAD_CORES_X86: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// WCSER-REHOME — which core currently carries the render service's singleton role.
///
/// Deliberately NOT `smp::render_cpu()`. That is the core the boot-time split PUBLISHED, and the
/// whole point of a re-home is that the role moves off it; reading the published split would make a
/// second death — of the rescue core — look like the death of a core that carried nothing, and the
/// recovery would silently not happen the second time. `usize::MAX` until the handoff spawns.
#[cfg(target_arch = "x86_64")]
static RENDER_ROLE_CPU_X86: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(usize::MAX);

/// WCSER-REHOME — THE RENDER ROLE'S OWNER EPOCH. Bumped once per re-home, before the replacement is
/// spawned. Each render instance captures it at entry and re-checks it every pass; an instance whose
/// epoch no longer matches has been SUPERSEDED and retires itself.
///
/// **This is the revenant check, generalised from a resource to a ROLE.** `comp_gate_release` already
/// refuses to release `COMP_GATE` unless this core is still `COMP_HOLDER_CORE` — a woken corpse
/// declines instead of handing a second compositor the panel. The same hazard exists one level up and
/// is strictly worse there: a corpse that wakes and resumes its render loop becomes a SECOND consumer
/// of `GUI_CHANNEL_X86` and a second owner of the panel present, which is exactly the state
/// `fbcon::detach()` exists to prevent. Every event it then took would be an event the real incumbent
/// never saw — silent, intermittent input loss with nothing on the wire.
///
/// **"The dead core never comes back" is an observation, not an invariant.** Boot 16 shows core 1's
/// `dec=` nibble at 0 across all 494 post-steal samples, which is evidence it never ran again, not a
/// guarantee that a stuck BAR store cannot eventually retire. `COMP_REVENANTS` exists precisely
/// because this tree already declined to build on that assumption once. The check is two relaxed
/// loads per pass; the assumption is a silent double-consumer.
///
/// An EPOCH rather than a core index, so the discipline stays correct even if a role were ever
/// re-homed back onto a core it had previously occupied. Written only by `render_rehome_service`,
/// which runs on one task (the pump) and is fed by a once-per-death mailbox, so the bump and the
/// spawn it precedes cannot interleave with another re-home.
#[cfg(target_arch = "x86_64")]
static RENDER_ROLE_EPOCH_X86: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// WCSER-REHOME — set across the spawn of a RESCUE render instance, and consumed by that instance on
/// entry. A rescue instance skips `open_shell_window`: the dead instance's shell row is still
/// registered and its surface memory is still mapped (a parked core frees nothing), so minting a
/// second row would put two shell windows on the desktop and pay ~5 MB for the privilege.
#[cfg(target_arch = "x86_64")]
static RENDER_RESCUE_X86: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// WCSER-REHOME — re-home the singleton roles of any core the steal has just declared dead.
///
/// Called from `x86_usb_pump`'s service pass, and that placement is the same one `DESKTOP-APP` uses
/// for the same three reasons: `spawn` allocates a task and a kernel stack and takes a run-queue
/// lock, so it must run from a scheduled task and never from inside the composite pass where the
/// steal happens; the pump is on the SERVICE core, which the steal has never killed; and only a
/// repeating pass can notice a death that happens at an arbitrary later second. Two relaxed loads
/// per pass when nothing is owed.
#[cfg(target_arch = "x86_64")]
fn render_rehome_service() {
    use core::sync::atomic::Ordering::{Relaxed, SeqCst};
    // `take`, not a read: exactly one caller ever sees a given death, so exactly one replacement is
    // ever spawned for it.
    let Some(dead) = unaos_kernel::video::wm::comp_dead_core_take() else {
        return;
    };
    if dead < 64 {
        DEAD_CORES_X86.fetch_or(1u64 << dead, Relaxed);
    }

    // Did this core carry the one role we re-home? A `no` is still worth a line: the census is the
    // deliverable, and a silent return would make "nothing was owed" and "we forgot" identical.
    if RENDER_ROLE_CPU_X86.load(Relaxed) != dead {
        serial_println!(
            ":: [wcser] REHOME: c{} is dead but carried no singleton role this kernel re-homes \
             (render role is on c{}); the core and any in-flight window are the accepted trade ::",
            dead,
            RENDER_ROLE_CPU_X86.load(Relaxed)
        );
        return;
    }

    // A rescue core must be neither the dead core nor the SERVICE core. `xhci_worker_cpu` already
    // enforces exactly that exclusion, and for the reason that matters here: the render service is a
    // preemptible taker of `XHCI_CONTROLLER` and so is `x86_usb_pump`, and two preemptible takers of
    // a raw spinlock on one core deadlock it. Re-homing the render role onto the service core would
    // trade a dead channel for a dead machine. So we ask the placement authority rather than
    // re-deriving it.
    //
    // **`DEAD_CORES_X86` is NOT belt-and-braces here — without it this loop can hand back the corpse.**
    // `smp::ONLINE_APS` is written exactly once, at `start_aps` time, and NOTHING EVER REMOVES A CORE
    // FROM IT: a core stays a placement candidate forever, however dead it is. The placement helpers
    // exclude the render and service cores by ROLE, and the render core is the very core we are
    // replacing — so on a machine where it is also the only pool candidate, an unguarded
    // `xhci_worker_cpu(0)` would answer with the core that just died and the "recovery" would spawn a
    // task onto a core that never dispatches. Silent, and indistinguishable from success on the wire.
    // So placement is EXPLICIT: ask the authority, then reject any answer we know to be a corpse.
    let mut rescue = None;
    for n in 0..unaos_kernel::arch::smp::online_aps().len() {
        match unaos_kernel::arch::smp::xhci_worker_cpu(n) {
            Some(c) if c < 64 && DEAD_CORES_X86.load(Relaxed) & (1u64 << c) == 0 => {
                rescue = Some(c);
                break;
            }
            Some(_) => continue,
            None => break, // the pool is exhausted or co-location is forbidden; both mean decline
        }
    }
    let Some(rescue) = rescue else {
        // DECLINE, LOUDLY. Co-locating is a documented deadlock and inventing a core is not an
        // option, so the honest answer is that this machine cannot recover — said out loud, on the
        // one transport a wedged boot still has, rather than papered over with a placement that
        // hangs. The input path stays ungated either way: INPUT-UNGATE means the producer is still
        // draining `pal::EVENT_QUEUE` and the pump still has its CPU.
        serial_println!(
            ":: [wcser] REHOME DECLINED: c{} carried the render role and there is no live core that \
             is neither it nor the service core — co-locating on the service core is the \
             XHCI_CONTROLLER deadlock, so the role stays down == tripwire ::",
            dead
        );
        return;
    };

    // Publish the new home BEFORE the spawn, so a second death racing us reads the right role owner.
    //
    // The EPOCH bump is what retires the corpse if it ever wakes: it is published here, ahead of the
    // replacement, so the incumbent-check at the top of every render pass is already armed by the
    // time either instance can run it. `Release` pairs with the `Acquire` the new instance uses to
    // capture its own epoch at entry.
    RENDER_ROLE_EPOCH_X86.fetch_add(1, core::sync::atomic::Ordering::Release);
    RENDER_ROLE_CPU_X86.store(rescue, Relaxed);
    RENDER_RESCUE_X86.store(true, SeqCst);
    unaos_kernel::arch::sched::spawn(
        "render-rehomed",
        x86_render_service,
        rescue,
        rescue,
        unaos_kernel::arch::sched::PRIO_NORMAL,
    );
    unaos_kernel::deadman::note_rehome();
    serial_println!(
        ":: [wcser] REHOMED the render role from DEAD c{} to c{} — GUI_CHANNEL_X86 has a consumer \
         again and the input path is ungated; c{} and its in-flight window stay lost == tripwire ::",
        dead,
        rescue,
        dead
    );
}

/// PTRCH — relative-motion events SUMMED into the event in front of them by the render service's
/// drain rather than dispatched on their own (see the fold in `x86_render_service`).
///
/// Deliberately NOT subtracted from `GUI_RECV_X86`, and for the same conservation reason PTRDEAD
/// keeps `EVQ_COALESCE_PTR` out of `EVQ_PUSH_PTR`: `sent - recv` is read as the channel's LIVE
/// OCCUPANCY, and a folded event HAS left the channel, so it must be counted as received. This is a
/// separate term describing what the consumer then did with it — and a nonzero reading is the
/// operator's answer to "did the render core fall behind the pad": it is the count of reports the
/// arrow was handed all at once instead of walking.
#[cfg(target_arch = "x86_64")]
static GUI_FOLD_X86: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// PTRCH — take one event OFF `GUI_CHANNEL_X86` without parking, charge the depth ledger, and show it
/// to the `usbdebug` witness. The single non-blocking choke point, so `GUI_RECV_X86` can never
/// disagree with the real receive count and no path can pull an event past the instrument.
#[cfg(target_arch = "x86_64")]
fn gui_try_recv_x86() -> Option<unaos_kernel::pal::Event> {
    let ev = GUI_CHANNEL_X86.try_recv()?;
    GUI_RECV_X86.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    #[cfg(all(feature = "usbdebug", feature = "wc"))]
    usbdebug_event_print(ev);
    Some(ev)
}

/// PTRCH — the blocking twin of [`gui_try_recv_x86`]: park until an event arrives, then charge the
/// same ledger and show it to the same witness.
#[cfg(target_arch = "x86_64")]
fn gui_recv_blocking_x86() -> unaos_kernel::pal::Event {
    let ev = GUI_CHANNEL_X86.recv();
    GUI_RECV_X86.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    #[cfg(all(feature = "usbdebug", feature = "wc"))]
    usbdebug_event_print(ev);
    ev
}

/// One-shot guard: log "RX interrupt live" exactly once, from the input task (never the ISR).
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static RX_LOGGED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// PIUSB-24: ms() timestamp of the last `[piusb24]` pointer witness line, to rate-limit it to ~4 Hz
/// (a moving mouse emits reports far faster than serial should mirror). 0 = never logged.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static PIUSB24_LAST_LOG_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// PIUSB-26: ms() timestamp of the last `[piusb26]` pump idle-cost witness, to rate-limit it to once
/// every ~5 s (the pump runs ~250×/s — no point mirroring every pass). 0 = never logged.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static PIUSB26_LAST_LOG_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// UVUG-5: ms() timestamp of the last `[el0in]` router-delivery witness. The metal `no interactive takeover`
/// question (P47) is un-reproducible in QEMU (no HID), so this line PROVES on the next sitting whether real
/// HID edges actually reached the active user app's input ring — `route_input_to_active_el0` returning >0 is
/// the router delivering keys/mouse the user app's SYS_INPUT_POLL then drains. Rate-limited to ~2 Hz so a
/// held key or a moving mouse never floods serial. 0 = never logged.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static EL0IN_LAST_LOG_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

// UVUG-5 / typematic — host-side key repeat. A USB HID boot keyboard under SET_IDLE(0) (which we arm, so a
// held key sends ONE press report and NO further reports until release — the GAME-MODE held-state contract
// depends on it) never auto-repeats. So a held key produced exactly one `Event::Key` everywhere in the stack:
// the shell line editor advanced one char and stopped, and Peter noticed the dead repeat at the bench. Key
// repeat is therefore the HOST's job. We synthesise it in the USB pump (the one periodic seam that already
// runs ~250×/s and owns `EVENT_QUEUE`): track the most-recently-pressed key from the Key/KeyUp EDGES the HID
// decode already emits, and once it has been held past an initial delay, push a fresh `Event::Key` into
// `pal::EVENT_QUEUE` at the pump's repeat rate. Injecting into EVENT_QUEUE means the repeat rides the SAME
// routing every real key takes — the shell path (GUI_CHANNEL), a kernel full-screen app's own pump_and_poll
// drain, AND a user app's per-process ring — with no per-path code. Newest key wins (standard typematic);
// releasing the repeating key stops it; a different key's release is ignored. QEMU raspi4b delivers no HID, so
// no key is ever held and no repeat is ever synthesised — the deterministic auto paths stay byte-identical.
//
// UVUG-6 moved the tracker STATE and logic into `pal` (the kernel lib) and re-rooted its observation at the
// HID REPORT level: `drivers::xhci` calls `pal::typematic_note_report` before any EVENT_QUEUE push, and this
// pump calls `pal::typematic_tick`. See `pal.rs` for the root-cause writeup (a `KeyUp` dropped by the full
// 64-slot ring used to strand a held key forever) and the three-layer disarm + backpressure guard that fix it.
// The former drain-fed `typematic_observe` is gone — observing the queue drain was the hole.

/// PIUSB-28: latched once the first Pi pump pass has armed the FAT mount trigger, so the
/// `:: piusb28: mount-trigger armed (pi pump path) ::` witness prints exactly once per boot. This
/// makes the wiring itself visible on serial — proving the mount edge is now polled from a path that
/// actually runs on Pi baremetal+fb (`usb_pump`/`input_service` poll-fallback), unlike the dead
/// main/GUI loop where PIUSB-27's original call sites live.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static PIUSB28_ARMED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// GUI-CLICK-1: previous pointer-button bitmask, so the render task acts on PRESS edges only (a new
/// bit going 0→1) and ignores the matching release. A raw HID button report carries the full set of
/// currently-held buttons, so without edge detection a press+release would dispatch twice (or a held
/// drag would re-fire every report). 0 = no button held.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static CLICK1_PREV_MASK: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// GUI-CLICK-1: ms() timestamp of the last `[click1]` witness line, to rate-limit it to ~10 Hz. A
/// press edge is rare compared to motion, but a chattering switch or a fast double-click should not
/// flood serial; genuine distinct clicks are far enough apart to survive the throttle. 0 = never.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static CLICK1_LAST_LOG_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// GUI-CLICK-2: true while a full-screen app owns the screen — set around `dispatch_command` in
/// `handle_key`. While true, `pump_usb_into_gui` STOPS forwarding pointer/key events into
/// GUI_CHANNEL and leaves them in `pal::EVENT_QUEUE` for the app's own `pump_and_poll` drain (vug,
/// pulse, …). This both delivers input (incl. the exit click) to the app AND stops the pump from
/// saturating the 64-slot GUI_CHANNEL while render_service is blocked inside the app — the P-metal
/// fps-decay-under-vug mechanism. Ungated (shared `handle_key` sets it on every arch); only the Pi
/// pump reads it. `[click1]` stays the no-app fallback dispatch.
static SCREEN_APP_ACTIVE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// GUI-CLICK-2 (depth witness): running count of events forwarded INTO GUI_CHANNEL and RECEIVED out
/// of it. `sent - recv` is the live channel queue-depth (the coordinator's saturation suspect). Both
/// send and recv sites live in this file's aarch64+baremetal service tasks, so the pair is exact
/// without touching the `Channel` type in sched.rs (another lane). Printed by the `[click2] depth`
/// line every ~5 s, always (so the degradation curve is on serial regardless of app-active state).
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static GUI_SENT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static GUI_RECV: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// GUI-CLICK-2: `[click2] depth` is rate-limited by a PASS COUNTER, not wall-clock: the pump runs
/// from the raspi4b poll-nap fallback where `arch::ms()` is stuck at 0 (Group-1 timer not live), so
/// an ms() rate-limit never holds there and floods serial. A pass counter is monotonic regardless of
/// timer state — one line every `CLICK2_DEPTH_EVERY` passes (~5 s at the metal ~250 Hz pump cadence).
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static CLICK2_DEPTH_PASSES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
const CLICK2_DEPTH_EVERY: u64 = 1250;
/// GUI-CLICK-2: ms() of the last `[click2] input left for app` witness (rate-limit ~5 s so a held/
/// chattering click during an app does not flood serial). Edge-triggered on a real press, which only
/// occurs on metal (QEMU raspi4b has no USB HID) where ms() advances — so ms gating is safe here.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static CLICK2_LEFT_LAST_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// GUI-CLICK-2: forward one event into GUI_CHANNEL and bump the sent counter (depth accounting). The
/// single choke point for every producer in this file, so `GUI_SENT` can never drift from real sends.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn gui_send(ev: unaos_kernel::pal::Event) {
    GUI_SENT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    GUI_CHANNEL.send(ev);
}

/// USBDBG-INVERT — the one line that names the regime on the wire, printed at the x86 GUI takeover.
///
/// THE CLAIM IT MAKES. This build carries `usbdebug` AND `wc`, and it did NOT stop in the terminal
/// bring-up loop: it is handing the panel to the ordinary desktop with the debug instruments riding
/// inside it. That is the whole content of the inversion, so it is one literal line, emitted from the
/// same seam that records `gui:handoff` — the last moment before `fbcon::detach` — and therefore
/// visible on the panel of a serial-less metal card as well as in a headless capture. It is the
/// SPIRIT of the retired `USBDBG-CURSOR: ... ARMED` / `USBDBG-ROUTE: ... ARMED` witnesses, which
/// existed to prove from the wire that a debug card had a working pointer and working clicks: those
/// two facts are no longer this build's to prove, because on the inverted path they are the desktop's
/// own (`x86_render_service` moves the sprite and `wc_route_event` routes the click for every x86
/// build alike). What IS this build's to prove is that it reached that desktop at all.
///
/// Latched, so a boot that somehow crossed the seam twice still prints once, and cheap: one relaxed
/// swap on the single takeover pass.
#[cfg(all(target_arch = "x86_64", feature = "usbdebug", feature = "wc"))]
fn usbdebug_invert_witness() {
    static PRINTED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    if !PRINTED.swap(true, core::sync::atomic::Ordering::Relaxed) {
        serial_println!(
            ":: USBDBG-INVERT: debug instruments riding the real desktop == witness ::"
        );
    }
}

/// USBDBG-INVERT — the `USB-DEBUG:` event view, relocated from the terminal loop onto the drains the
/// real desktop actually runs (`x86_render_service`, and the inline BSP GUI loop taken when fewer
/// than two APs came online).
///
/// PRINT **AND** ROUTE, IN THAT ORDER, KEYED ON THE RAW REPORT. The old loop printed instead of
/// routing, which is the defect Peter's ruling names; here the call sits ahead of `wc_route_event` and
/// consumes nothing, so the event goes on to be routed exactly as it is on a build without the knob.
/// Keying on the RAW report rather than the routed outcome is what keeps the instrument HONEST: a
/// report consumed into a focused ring-3 window's ring comes back `Event::Unknown`, so a routed-keyed
/// print would fall silent for precisely the input that is working — the operator would read "no
/// events" off a card whose events are being delivered. Raw is what the hardware sent, which is the
/// question a bring-up card is asked.
///
/// `Event::Button` is printed here and was NOT printed by the loop this replaces — the loop's `match`
/// dropped presses into `_ => {}`, which is the hole USBDBG-ROUTE was chasing on boot 5. A card whose
/// clicks are the thing under investigation should say so on the wire.
///
/// The KEY / MOUSE line formats are carried over verbatim, so every eye and every `awk` that reads a
/// diagnosis capture keeps working across the inversion.
///
/// ### PTRWIT — **the pointer witness had become the biggest thing on the wire, and it was starving
/// the pointer it was watching.**
///
/// Measured, boot 10 of `rmbp2-boot8` (the `UNAOS_USBDEBUG=1` desktop Peter flies): `USB-DEBUG: MOUSE
/// relative …` appeared 2456 times for **121,164 bytes — 20.8 % of the entire boot's serial output,
/// the single largest producer in the capture, ahead of `[wc-h] rollup` at 98 kB.** Mean line length
/// with the `logts` prefix and the newline: **49.33 bytes**, i.e. **4.28 ms of 115200-baud line time
/// per pointer report**. The pad's own cadence in that same slice is 8 ms flat (p50 gap = 8 ms), so
/// **over half of every pointer interval was spent printing the pointer** — and the witness therefore
/// could not keep up with the thing it existed to witness: 6.2 kB/s of pointer text against an 11.5
/// kB/s wire, before a single other subsystem printed anything.
///
/// That is not an instrument observing a system, it is an instrument loading it. The standing ruling
/// is that a diagnosis card runs the REAL desktop, which makes the witness's cost part of the
/// desktop's behaviour — so the cost has to come down without the diagnosis going away.
///
/// ### What replaces it, and what it still answers
///  * **The first [`USBDBG_PTR_VERBATIM`] relative reports of the boot print in the OLD FORMAT, byte
///    for byte.** That is the bring-up question — "did a real report arrive, and what did it say" —
///    and it is asked once per boot, not 125 times a second. Every `awk` written against
///    `USB-DEBUG: MOUSE relative dx=` keeps matching.
///  * **After that, motion FOLDS into a rollup** emitted at most every [`USBDBG_PTR_ROLLUP_MS`], and
///    flushed immediately ahead of any non-motion event so the wire stays chronological: a drag reads
///    `MOUSE rel n=… ` then `BUTTON mask=0x00` in that order, which is what makes a gesture legible.
///    The rollup carries the two facts the steady state is actually read for — **how many reports
///    arrived and over what span** (i.e. the cadence, which is where a dead zone shows up) — plus the
///    summed travel, which is exactly what the queue would have folded anyway (`pal`'s PTRDEAD).
///  * `Event::Timer` — the input service's 250 ms pulse — lands in the `_` arm and flushes, so the
///    LAST rollup of a gesture reaches the wire promptly instead of waiting for the next motion. The
///    pulse rate and the rollup rate are the same 250 ms by construction, not by coincidence.
///
/// KEY, BUTTON and absolute-motion lines are untouched and stay per-event: they are human-rate
/// (a keystroke, a click, a QEMU tablet report), they are never the amplifier, and they are the lines
/// a bring-up card is read for.
///
/// **The arithmetic.** 49.33 bytes/report -> one 64-byte rollup per 250 ms, i.e. 2.05 bytes/report at
/// the pad's 125 Hz: **4.28 ms -> 0.18 ms of line time per report, a 24x reduction**, and the pointer
/// path's steady-state share of the wire drops from 54 % of the report interval to 2.2 %.
#[cfg(all(target_arch = "x86_64", feature = "usbdebug", feature = "wc"))]
fn usbdebug_event_print(raw: unaos_kernel::pal::Event) {
    use core::sync::atomic::Ordering::Relaxed;
    match raw {
        unaos_kernel::pal::Event::Key(c) => {
            usbdebug_ptr_rollup_flush();
            let ch = c as char;
            serial_println!("USB-DEBUG: KEY {:#04x} '{}'", c, if c >= 32 && c < 127 { ch } else { '.' });
        }
        unaos_kernel::pal::Event::Mouse { x, y } => {
            // The bounded verbatim prologue: the old line, unchanged, for the first reports of the
            // boot. `fetch_add` on a saturating compare rather than a swap so the two mutually
            // exclusive drains (the render service and the inline BSP loop) share one budget.
            if PTR_VERBATIM_SEEN.load(Relaxed) < USBDBG_PTR_VERBATIM {
                PTR_VERBATIM_SEEN.fetch_add(1, Relaxed);
                serial_println!("USB-DEBUG: MOUSE relative dx={} dy={}", x, y);
                return;
            }
            let now = unaos_kernel::arch::ms();
            if PTR_ROLL_N.fetch_add(1, Relaxed) == 0 {
                PTR_ROLL_FIRST_MS.store(now, Relaxed);
            }
            PTR_ROLL_DX.fetch_add(x as i64, Relaxed);
            PTR_ROLL_DY.fetch_add(y as i64, Relaxed);
            if now.wrapping_sub(PTR_ROLL_FIRST_MS.load(Relaxed)) >= USBDBG_PTR_ROLLUP_MS {
                usbdebug_ptr_rollup_flush();
            }
        }
        unaos_kernel::pal::Event::MouseAbsolute { x, y } => {
            usbdebug_ptr_rollup_flush();
            serial_println!("USB-DEBUG: MOUSE absolute x={} y={}", x, y);
        }
        unaos_kernel::pal::Event::Button(mask) => {
            usbdebug_ptr_rollup_flush();
            serial_println!("USB-DEBUG: BUTTON mask={:#04x}", mask);
        }
        // `Event::None` is the inline BSP drain's END-OF-QUEUE SENTINEL, not an event — `pal.poll_event()`
        // hands it back once per frame with nothing behind it. It must NOT flush: on that drain a
        // flush-on-None would fire once per FRAME, turning a 250 ms rollup into a 60 Hz one and giving
        // back most of what PTRWIT just saved. It is also the one variant here that is not a report.
        unaos_kernel::pal::Event::None => {}
        // Timer (the 250 ms pulse), KeyUp, Unknown — real reports with nothing of their own to print,
        // and the seam that gets the tail of a gesture onto the wire while the hand is still.
        _ => usbdebug_ptr_rollup_flush(),
    }
}

/// PTRWIT — relative motion reports printed VERBATIM, in the pre-rollup format, before the fold
/// starts. One per report is affordable exactly once per boot; it is the steady state that is not.
#[cfg(all(target_arch = "x86_64", feature = "usbdebug", feature = "wc"))]
const USBDBG_PTR_VERBATIM: u32 = 16;

/// PTRWIT — the rollup's minimum spacing. 250 ms, which is [`X86_GUI_PULSE_MS`]: the input service
/// already posts an `Event::Timer` at that cadence and that Timer flushes this rollup, so a second,
/// different period here would only ever produce a ragged line rate.
#[cfg(all(target_arch = "x86_64", feature = "usbdebug", feature = "wc"))]
const USBDBG_PTR_ROLLUP_MS: u64 = X86_GUI_PULSE_MS;

/// PTRWIT — reports the verbatim prologue has spent (see [`USBDBG_PTR_VERBATIM`]).
#[cfg(all(target_arch = "x86_64", feature = "usbdebug", feature = "wc"))]
static PTR_VERBATIM_SEEN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// PTRWIT — relative reports folded into the pending rollup. 0 = nothing pending, and the flush is
/// a single relaxed load on every event that is not motion.
#[cfg(all(target_arch = "x86_64", feature = "usbdebug", feature = "wc"))]
static PTR_ROLL_N: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// PTRWIT — summed dx / dy of the pending rollup. `i64` because the sum is unbounded in principle and
/// a wrapped total would read as travel in the wrong direction.
#[cfg(all(target_arch = "x86_64", feature = "usbdebug", feature = "wc"))]
static PTR_ROLL_DX: core::sync::atomic::AtomicI64 = core::sync::atomic::AtomicI64::new(0);
#[cfg(all(target_arch = "x86_64", feature = "usbdebug", feature = "wc"))]
static PTR_ROLL_DY: core::sync::atomic::AtomicI64 = core::sync::atomic::AtomicI64::new(0);
/// PTRWIT — `ms()` of the FIRST report folded into the pending rollup, so the line's `span=` is the
/// real interval those reports arrived over and `n/span` is the pad's measured cadence.
#[cfg(all(target_arch = "x86_64", feature = "usbdebug", feature = "wc"))]
static PTR_ROLL_FIRST_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// PTRWIT — emit the pending relative-motion rollup, if there is one, and reset it.
///
/// Idempotent and free when nothing is pending (one relaxed load), which is what lets it be called
/// unconditionally from every non-motion arm — the ordering guarantee ("the rollup is always on the
/// wire AHEAD of the edge that ended it") is worth more than the branch it costs.
#[cfg(all(target_arch = "x86_64", feature = "usbdebug", feature = "wc"))]
fn usbdebug_ptr_rollup_flush() {
    use core::sync::atomic::Ordering::Relaxed;
    let n = PTR_ROLL_N.swap(0, Relaxed);
    if n == 0 {
        return;
    }
    let dx = PTR_ROLL_DX.swap(0, Relaxed);
    let dy = PTR_ROLL_DY.swap(0, Relaxed);
    let span = unaos_kernel::arch::ms().wrapping_sub(PTR_ROLL_FIRST_MS.load(Relaxed));
    serial_println!(
        "USB-DEBUG: MOUSE rel n={} dx={} dy={} span={}ms",
        n, dx, dy, span
    );
}

/// SERIAL-FOCUS — at most ONE wake token from the serial console is outstanding in `GUI_CHANNEL` at
/// any instant. Set when `serial_to_shell` posts one, cleared by the render task's drain.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static SERIAL_WAKE_PENDING: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// SERIAL-FOCUS — how much of the 64-slot `GUI_CHANNEL` must be free before the serial console is
/// allowed to post its wake token. 16 slots of headroom against a producer set of exactly three
/// (`usb_pump`'s forward, `status-tick`'s 1 Hz pulse, and this one token) makes the `send` below
/// non-blocking by measurement rather than by hope — and when the headroom is not there the token is
/// simply not sent, which costs latency and never a wait.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
const SERIAL_WAKE_HEADROOM: u64 = 16;

/// SERIAL-FOCUS — **THE PRODUCER SIDE OF THE SOURCE SPLIT.** One byte off the PL011, on its way to
/// the shell and to nowhere else.
///
/// This replaces `gui_send(Event::Key(byte))`, which was the whole blocker (see the module header on
/// `arch::aarch64::serial::shell_inbox` for the two-part failure it produced). The byte goes into
/// the shell's own bounded inbox, which is not focus-routed and has no blocking edge; `GUI_CHANNEL`
/// now carries no serial PAYLOAD at all, only — at most, and only when it provably cannot block — a
/// single coalesced WAKE token to unpark the render task from `recv`.
///
/// THE THREE REASONS A TOKEN IS WITHHELD, and why each one is safe rather than a dropped keystroke:
///
///   * ALREADY PENDING. A token is in flight; the drain it will trigger takes the whole ring, not
///     one byte. This is what makes a storm cost the channel ONE slot no matter how many bytes
///     arrive — the property the brief asks for, and the reason a serial storm cannot jam the GUI
///     channel: the storm never travels on that channel.
///   * `SCREEN_APP_ACTIVE`. The render task is parked inside `dispatch_command` and cannot `recv` at
///     all; a token would sit in the channel unread. The drain in `render_service` is placed AFTER
///     the `match`, so the pass that returns from `dispatch_command` drains the backlog before it
///     parks again — delivery is immediate on the app's exit without any token.
///   * NO HEADROOM. The channel is fuller than `SERIAL_WAKE_HEADROOM` allows. A `send` here is the
///     only blocking call on this path and this is the check that refuses it. The bytes wait; the
///     next event of any kind (a pointer report, a USB key, the ~1 Hz strip `Timer`) runs the drain.
///
/// In every withheld case the bytes are already SAFE in the ring, in order, and the flag is left
/// false so the next byte re-tries the wake. Nothing is lost by withholding; only latency is spent.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn serial_to_shell(byte: u8) {
    use core::sync::atomic::Ordering;
    if !unaos_kernel::arch::serial::shell_inbox::offer(byte) {
        return; // ring full — dropped and counted; the `[serfocus]` census says so on the wire
    }
    if SCREEN_APP_ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let depth = GUI_SENT
        .load(Ordering::Relaxed)
        .wrapping_sub(GUI_RECV.load(Ordering::Relaxed));
    if depth + SERIAL_WAKE_HEADROOM >= GUI_CHANNEL_CAP {
        return;
    }
    if !SERIAL_WAKE_PENDING.swap(true, Ordering::AcqRel) {
        // `Event::Timer` deliberately, rather than a new variant: the render task already handles it
        // as "recompose the strip if its content changed", `ui_status::tick` paces that to 1 Hz
        // internally so a burst costs nothing, and the shared `pal::Event` enum — which every arch
        // and both routers read — gains not one line for this arc.
        gui_send(unaos_kernel::pal::Event::Timer);
    }
}

// =================================================================================================
// INWEDGE — the input core must never leave the panel lock held across a dispatch
// =================================================================================================
//
// THE WEDGE THIS CLOSES (dsktp boot 8, `pi4-pi1-b1/ttyACM0.log`). Core 3 — the INPUT core, host to
// `input`, `usb-pump`, `rx-backstop` and `status-tick` — stopped dead with `[spin1] cpu=3
// task=99:input ... sched phase=6 passes=2860` and EVERY counter byte-identical across 21 prints
// spanning 14.8 s to 465.3 s: `passes` frozen, `disp busy/idle` frozen, and `irq total=3081
// last=30` frozen. INTID 30 is the generic-timer PPI: the last thing that core did was take a timer
// interrupt, and it never left it. A frozen IRQ total is the masked-spin signature SPIN-7/SPIN-8
// established, and phase 6 (`SPIN8_TASK`) says the core was inside a task, not inside its own
// scheduler loop. The three already-witnessed masked spins all read CLEAN on that capture
// (`sem_stalls=0`, `futex_stalls=0`, no `[wedge4] preempt-in-section` line), so the spin was on a
// raw `spin::Mutex` that none of WEDGE-4/5/6 watches.
//
// THE INTERLEAVING, on ONE core:
//
//   1. `usb-pump` (input core, PRIO_SERVICE) takes a HID pointer report into
//      `route_input_to_active_el0` and runs the FOCUS-VIS cursor keep-alive, which reads panel
//      geometry out of `video::WRITER` and then repaints the sprite through `video::cursor`. Both
//      acquire `WRITER`, and — this is the defect — they acquired it BLOCKING and with interrupts
//      ENABLED, from a preemptible kernel task.
//   2. The quantum tick lands inside that section. `timer_preempt` marks the task READY and switches
//      to the scheduler, which requeues it. The task is now off-CPU **still holding `WRITER`**.
//   3. The core dispatches its band peer, `input`.
//   4. The NEXT timer tick lands with `input` current. `timer_preempt` calls `load_witness_tick`,
//      whose emit is a `serial_println!` — and that print's panel mirror takes `WRITER` with a
//      BLOCKING acquire, from inside the IRQ vector, IRQ-masked.
//      LOCKFIX — NAMING THE ACQUIRE CORRECTLY. The original text credited this to `video::fbcon::_print`.
//      It is not: `fbcon::_print` takes `FBCON.try_lock` (`video/fbcon.rs`) and `serial::_print` takes
//      `SERIAL_PORT.try_lock` — neither can block. The blocking `WRITER.lock()` on the print path is in
//      `wm::composite` (`video/wm.rs`), reached via `route_present_rows` → `wm::present` when the panel
//      console owns the glass, i.e. on `desktop_firmware`/`wc` builds. The mechanism, the interleaving and the fix
//      are unchanged — only the callee's name was wrong, and a wrong name sends the next reader to a
//      file that is already safe. `docs/dev/OS/…/scheduler.md` §INWEDGE carries the same correction.
//   5. The acquire never returns. The core is masked, so the holder it displaced can never be
//      redispatched to release the lock, and `SCHED[3].current` still names the interrupted task —
//      `99:input`. Every task pinned to that core dies with it: `usb-pump` emitted `[piusb26]`
//      exactly ONCE in the whole boot (it is rate-limited to 5 s), `rx-backstop` froze at
//      `bs_phase=1 bs_loops=2`, and no `[wheel1]`, `[piusb24]`, `[el0in]` or `[cursor] armed` line
//      ever appeared. That is the whole input subsystem, ~15 s into the boot.
//
// THE RULE, stated once: **the input router may not hold a raw panel lock across a dispatch, and may
// not block on one.** Every other raw-spinlock section on this arch already obeys it — `sched::rq`
// masks, `Semaphore`/`FutexBucket` mask, `shell_inbox` masks, the global allocator masks. The input
// router was the exception, and it is the one task set that shares a core with the kernel's only
// IRQ-context printer.
//
// The two hint readers below therefore acquire MASKED (so the section cannot be preempted) and
// NON-BLOCKING (so the router can never itself be the spinner). A refused read yields 0, which is the
// clamp-to-(0,0) degradation the FOCUS-VIS doc already sanctions for an unset framebuffer, and it is
// COUNTED — see `inwedge_note_refused`: a nonzero `[inwedge]` census on the wire is the boot-8 window
// being entered and SURVIVED, which is the recurrence witness this arc owes.

/// INWEDGE — `(read, refused)`.
///
/// LOCKFIX — the counters and the acquisition itself now live in `video` (`panel_census` /
/// `panel_info_nonblocking`), because the router is not the only input-path caller of them:
/// `quarry::live::wheel_route` and `syscall::click_pointer_pos` read the same geometry on the same
/// preemptible band and were still taking `WRITER` blocking. One door, one census, one rule. These
/// two functions stay so the witness and the selftest below read as they always have.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn inwedge_census() -> (u32, u32) {
    unaos_kernel::video::panel_census()
}

/// INWEDGE — the ONE panel-lock acquisition the input path makes: masked, non-blocking, counted.
/// Returns `None` when the lock is held elsewhere (or the framebuffer is unset); the callers clamp.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn inwedge_panel_info() -> Option<unaos_boot_info::FrameBufferInfo> {
    unaos_kernel::video::panel_info_nonblocking()
}

/// FOCUS-VIS — panel dimensions for the router's cursor keep-alive, read straight from the scan-out
/// framebuffer. The router has no `TargetPal` (that lives in the render task), and `pal::cursor` needs
/// bounds to clamp the hot spot against; `video::WRITER` is the same surface the sprite is drawn into,
/// so the two can never disagree. Zero while the framebuffer is unset, which clamps to (0,0) — harmless,
/// and unreachable in practice since a pointer report implies a booted display.
///
/// INWEDGE — and zero, now, also when the lock is CONTENDED. See the block above for why blocking here
/// is the wedge and refusing is not.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn pal_width_hint() -> i32 {
    inwedge_panel_info().map(|i| i.width as i32).unwrap_or(0)
}

/// FOCUS-VIS — see [`pal_width_hint`].
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn pal_height_hint() -> i32 {
    inwedge_panel_info().map(|i| i.height as i32).unwrap_or(0)
}

/// EL0IN-FOCUS: true only while `input_router_selftest` is inside `route_input_to_active_el0`. That test
/// pushes SYNTHETIC pointer events through the real router, and on a QEMU gate with no HID they would
/// otherwise be the first "pointer report" of the boot — arming the system cursor, painting a sprite and
/// printing `[cursor] armed` on a panel that has no pointer. The router's cursor keep-alive skips its work
/// while this is set, which is the narrowest possible form of the guard: every REAL report (there can be
/// none while the BSP is inside the selftest — the HID pump task is not spawned yet) moves the cursor.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static ROUTER_SELFTEST: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// INPUT-WIRE (ELF-5 router fold): drain every pending pal event into the ACTIVE user program's per-process
/// input ring via the `user_input_enqueue` seam. The single choke point for router->ring delivery — called by
/// `pump_usb_into_gui` when a user app holds input focus, and exercised directly by `input_router_selftest`.
/// Returns the count actually queued (a deliverable event on a non-full ring); Timer/None/Unknown and a full
/// ring are dropped by the seam (returns `false`). Never forwards into GUI_CHANNEL — that is the whole point.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn route_input_to_active_el0() -> usize {
    let mut routed = 0usize;
    while let Some(ev) = unaos_kernel::pal::next_event() {
        // UVUG-6: no drain-fed typematic observe here — the tracker is fed at the HID report level
        // (drivers::xhci -> pal::typematic_note_report), which a dropped queue event cannot defeat.
        //
        // FOCUS-VIS — the SYSTEM CURSOR IS SYSTEM-WIDE, so it must survive this branch. Everything
        // below routes the event onward to the focused app's ring and returns; the shell loop's
        // `Mouse`/`MouseAbsolute` arms — the ONLY code that moved the shared pointer state and repainted
        // `video::cursor` — are not reached at all while an app holds focus. The sprite therefore froze
        // where it was and auto-hid 1.5 s later, and no amount of mouse movement brought it back: the
        // reports were all going to the app. That is a system cursor disappearing because of who owns
        // the KEYBOARD, which is not a relationship that should exist.
        //
        // Delivery is unchanged — the event still goes to the app, which is what focus means; this only
        // keeps the kernel's own pointer state and its top-most sprite current alongside it.
        //
        // EL0IN-FOCUS — WHY THIS GATE IS NO LONGER `has_reported()`. FOCUS-VIS gated the keep-alive on
        // `pal::cursor::has_reported()` to keep the boot-time `input_router_selftest` — which drives this
        // exact function with a SYNTHETIC `Event::Mouse` against a fake focus — from arming a cursor and
        // printing `[cursor] armed` on a QEMU gate that has no pointer at all. The stated premise was that
        // "a machine with a real pointer has always armed it through the shell loop first (focus is the
        // shell at boot)". That premise is false whenever a user app takes focus BEFORE the first pointer
        // report of the boot — `run`/`bg` a windowed app, then touch the mouse — which is the ordinary
        // desktop bring-up. In that state `has_reported()` is still false, so this block is skipped; the shell arms (`render_service`'s `Mouse` arm, the only OTHER `move_rel` caller on this arch) are unreachable because `pump_usb_into_gui` took the `user_input_active() != 0` branch; and nothing else in the kernel can ever set the latch. So the predicate could only become true through a
        // path the predicate itself had disabled: the pointer stayed dead — no motion, no sprite — until
        // the operator TAB'd focus back to the shell, at which point the shell drain armed it and the
        // cursor came alive for the rest of the boot (P67v2, bench).
        //
        // The gate is therefore scoped to the thing it was actually protecting: the selftest's own call.
        // `ROUTER_SELFTEST` is true only for the few instructions that test spends inside this function,
        // so its synthetic events still arm nothing and the QEMU gate output is unchanged — while a REAL
        // pointer report moves the system cursor from the first report of the boot, whoever holds focus.
        if !ROUTER_SELFTEST.load(core::sync::atomic::Ordering::Relaxed) {
            // INWEDGE — THE SECTION, and the whole of it. `video::cursor::repaint` takes `WRITER`
            // itself (`refresh_locked`'s flush), so making only the two geometry reads safe would
            // leave the wider hold — the sprite restore+draw — still preemptible on the input core,
            // i.e. would leave the wedge exactly where boot 8 found it. Masking the section is what
            // makes the rule true rather than nearly true: the input router now cannot be switched
            // away from while it holds a panel lock, so no later masked acquirer on this core can
            // ever spin on one it cannot get released. This is the same discipline `sched::rq`,
            // `Semaphore`, `shell_inbox` and the global allocator already keep; the router was the
            // exception. The masked span is bounded and small — two `info()` reads and one 16x16
            // sprite restore+draw with its cache clean — and it runs at most once per HID report on a
            // ~250 Hz pump, which is why paying it here is cheaper than any lock-ordering scheme.
            //
            // LOCKFIX — AND THE SPAN NOW HOLDS TO THE RULE IT STATES. Masking the section made the
            // router unpreemptible; it did NOT, on its own, make the section safe to run masked, and
            // two things inside it were still forbidden there. (1) `repaint`'s WRITER takes were
            // BLOCKING acquires — a masked core spinning on a lock a preempted holder owns is the
            // boot-8 death with the roles reversed, i.e. the mask converted a recoverable stall into
            // an unrecoverable wedge, on every `desktop_firmware` boot. Those takes now go through
            // `video::panel_snapshot`, which blocks only when interrupts are enabled. (2) `repaint`
            // printed two witness lines and marked compositor damage from in here; `serial_println!`
            // on a `desktop_firmware` build mirrors to the panel through `wm::present` → `composite`, whose
            // `WRITER.lock()` is blocking, and `repair` takes the window TABLE lock. Both are now
            // handed back as a `RepaintTail` and run BELOW, with interrupts restored. What remains
            // inside the mask is pixels and atomics — no blocking lock, no printing.
            let tail = unaos_kernel::arch::without_interrupts(|| match ev {
                unaos_kernel::pal::Event::Mouse { x, y } if x != 0 || y != 0 => {
                    unaos_kernel::pal::cursor::move_rel(
                        x,
                        y,
                        pal_width_hint(),
                        pal_height_hint(),
                    );
                    unaos_kernel::video::cursor::repaint_deferred()
                }
                unaos_kernel::pal::Event::MouseAbsolute { x, y } => {
                    unaos_kernel::pal::cursor::set_abs(x, y, pal_width_hint(), pal_height_hint());
                    unaos_kernel::video::cursor::repaint_deferred()
                }
                _ => unaos_kernel::video::cursor::RepaintTail::default(),
            });
            tail.finish(); // LOCKFIX — damage + witness lines, UNMASKED. A no-op for the `_` arm.
        }
        #[cfg(feature = "desktop_firmware")] unaos_kernel::video::wm::drag_route_tail(ev); // DRAG-PI M3 — STEER a live title-bar grab from the FOCUSED-APP drain, after the cursor keep-alive above has applied this report and before the event is routed onward. This is the path a grab actually takes: the chrome arm focuses the dragged window's own owner, so every subsequent pointer report arrives here and is PACKED into that app's ring — the shell drain never sees it. Keyed on the raw event inside `drag_route_tail`, so a non-pointer event and an idle boot both cost one match and one atomic load. Delivery is unchanged: the app still receives the report, exactly as it does today.
        let queued = unaos_kernel::arch::aarch64::syscall::user_input_enqueue(ev);
        // WHEEL — account the wheel's fate at the ONE seam that knows it. `user_input_enqueue`
        // collapses "no EL0 target", "not deliverable" and "ring full" into a single `false`, and for
        // a `Wheel` the middle case is impossible (`pack_input` always packs it) — so a `false` here
        // means the delta was decoded correctly and had nowhere to land. That is precisely the case
        // the `[wheel1]` census exists to separate from "the byte was never decoded", because on a
        // machine where nothing scrolls the two are indistinguishable from the outside.
        if let unaos_kernel::pal::Event::Wheel(_) = ev {
            if queued {
                unaos_kernel::pal::wheel_note_routed();
            } else {
                unaos_kernel::pal::wheel_note_no_focus();
            }
        }
        if queued {
            routed += 1;
        }
    }
    // UVUG-5: prove router->ring delivery on metal. The `no interactive takeover` P47 symptom left it open
    // whether HID edges reached the user ring at all (QEMU can't test the HID edge). A rate-limited `[el0in]`
    // line here fires the instant the router hands the active user app real input, so the next sitting reads
    // delivery directly instead of inferring it. Silent when nothing routed (the common empty-pass case).
    if routed > 0 {
        use core::sync::atomic::Ordering;
        let now = unaos_kernel::arch::ms();
        let last = EL0IN_LAST_LOG_MS.load(Ordering::Relaxed);
        if now.wrapping_sub(last) >= 500 || last == 0 {
            EL0IN_LAST_LOG_MS.store(now.max(1), Ordering::Relaxed);
            serial_println!("[el0in] routed {} event(s) to active EL0 ring", routed);
        }
    }
    routed
}

/// INPUT-WIRE QEMU witness: prove the ROUTER path (EVENT_QUEUE -> the active-focus user ring) that the ELF-5
/// in-RAM `input_launcher` test cannot — it injects straight into `user_input_enqueue`, bypassing the router.
/// This runs the REAL router drain (`route_input_to_active_el0`, the exact code `pump_usb_into_gui` calls)
/// against a FAKE active focus and asserts: (1) a Key and a Mouse event pushed into EVENT_QUEUE are routed
/// to the focused ring (routed == 2), (2) a non-deliverable Timer is dropped (not routed), and (3) GUI_CHANNEL
/// is BYPASSED (GUI_SENT unchanged — the events did NOT leak into the render channel). The ring -> user drain
/// half is proven by the ELF-5 `:: EL0: input test … ::` witness; together they cover the full HID->user path.
/// HONEST QEMU NOTE: the real HID *edge* (a USB keypress landing in EVENT_QUEUE) is metal-only — QEMU raspi4b
/// delivers no HID — so this drives the router with a synthetically pushed event.
///
/// INROUTE — WHERE THIS RUNS, AND WHY IT MOVED. Called ONCE on the BSP from the `start_aps` block, BEFORE the
/// secondaries are started. That placement is load-bearing, not cosmetic.
///
/// The test borrows the two pieces of GLOBAL input state — `USER_INPUT_ACTIVE` (it fakes focus onto ASID 1)
/// and `pal::EVENT_QUEUE` (it pushes synthetic events and expects to be the one who drains them) — and then
/// COUNTS deliveries. Any concurrent owner of either one makes the count wrong. Its old home was next to the
/// input/render task spawn, under a comment claiming "EVENT_QUEUE is empty and no user slot is live". That was
/// simply untrue by then: the M6b..U7 fixture cascade is spawned far earlier and is running on the APs, and
/// `M6d` alone holds all eight slots — ASIDs 1 through 8. So the fake target ASID 1 was a REAL, LIVE slot
/// belonging to a fixture, and when that fixture exited mid-test its teardown ran
/// `clear_handle_row(1)` -> `USER_INPUT_ACTIVE.compare_exchange(1, 0)`, revoking the focus this test had just
/// set. A router pass that had already enqueued the Key then found no active target for the Mouse and
/// returned `routed=1`: the observed `:: USER: input router — routed=1|0 … :: FAIL ::`, ~1 boot in 7 under
/// contention. Nothing was dropped by the queue (`[uvug10] evq drop` stayed 0) and nothing leaked to
/// GUI_CHANNEL — the events were simply routed to a focus that no longer existed.
///
/// The revocation itself is CORRECT and is deliberately left alone: a torn-down slot must stop receiving
/// input, and the pre-launch discard in `user_input_set_active` is likewise correct for real input (an event
/// queued before an app existed was never meant for it — UVUG-8r2). The bug was the test's precondition, so
/// the fix is to make the precondition TRUE by construction: run before any user slot in this boot exists.
/// Serialising structurally beats the alternatives — giving the test a private sink would stop exercising
/// the real global seam, which is the entire point of the witness, and any "retry on mismatch" would just
/// launder a real race into a slower green.
///
/// The `[inroute]` line below is the standing evidence that the window stayed clean: it reports the focus
/// revocation count across the measurement window (`revokes=0` is the healthy reading) alongside the drained
/// counts, so a future regression that re-introduces a concurrent owner is diagnosable from the log alone
/// rather than by re-deriving the race.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn input_router_selftest() {
    use core::sync::atomic::Ordering;
    use unaos_kernel::arch::aarch64::syscall as sc;
    let sent0 = GUI_SENT.load(Ordering::Relaxed);
    // EL0IN-FOCUS: mark the whole measurement window as the selftest's, so the router's cursor keep-alive
    // ignores the synthetic pointer events below (no `[cursor] armed` on a panel with no pointer). Cleared
    // unconditionally before the verdict is printed; nothing here can early-return between the two.
    ROUTER_SELFTEST.store(true, Ordering::Relaxed);
    // INROUTE: bracket the measurement window with the focus-revocation counter (see `el0_focus_revokes`).
    let revokes0 = sc::el0_focus_revokes();
    // Fake focus: ASID 1 is a valid ring index (the per-ASID rings exist independent of slot liveness), and
    // at THIS call site no user slot has been allocated yet in this boot — the secondaries that run the fixture
    // cascade are not started until the line after this call returns, so nothing else can target ASID 1 or
    // revoke our focus (INROUTE; the `revokes=0` line below is the running proof). Setting focus resets the ring.
    sc::user_input_set_active(1);
    unaos_kernel::pal::push_event(unaos_kernel::pal::Event::Key(b'R'));
    unaos_kernel::pal::push_event(unaos_kernel::pal::Event::Mouse { x: 3, y: -4 });
    unaos_kernel::pal::push_event(unaos_kernel::pal::Event::Timer); // non-deliverable — must be dropped
    let routed = route_input_to_active_el0();
    let sent1 = GUI_SENT.load(Ordering::Relaxed);
    // UVUG-8r2 (a): DELIVERY IS NOT TAKEOVER. Two real events just landed in ASID 1's ring, and the latch must
    // still be 0 — takeover is engaged by the app CONSUMING an event via SYS_INPUT_POLL, not by the router
    // pushing one. Pre-r2 this latched here, which is exactly how the stale launch keystroke handed every
    // keyboard-started run a suspended deadline at t≈0. (The consume edge itself needs a live user task, so it
    // is metal-only; this asserts the half QEMU can see.)
    let push_does_not_engage = sc::el0_takeover_active() == 0;
    // UVUG-8r2 (b): STALE PRE-LAUNCH EVENTS ARE DISCARDED ON FOCUS. This is the metal `run /fat/VUG.ELF`
    // scenario in miniature: an event sits in EVENT_QUEUE from before the app existed (the Enter KeyUp that
    // launched it), then focus is granted. `user_input_set_active` must drain it, so the very next router pass
    // finds nothing to deliver — the app cannot mistake its own launch keystroke for in-app interaction.
    unaos_kernel::pal::push_event(unaos_kernel::pal::Event::Key(b'\n')); // the "launch" keystroke
    sc::user_input_set_active(1); // fresh focus — must discard it
    let stale_dropped = route_input_to_active_el0() == 0;
    // UVUG-8r2 (c): the pure decisions the wait loop uses, over a suspend -> WEDGE -> re-arm cycle. QEMU
    // raspi4b delivers no HID, so the live path cannot be driven here; these prove the logic instead.
    let deadline: u64 = 1_000;
    let stale: u64 = 200; // heartbeat staleness bound, in the same synthetic tick unit
    //   (S) latched on this asid with a FRESH heartbeat -> suspended, so even far past the deadline: no timeout.
    let live_suspends = sc::takeover_suspends(1, 1, 10_000, 9_900, stale);
    let suspend_holds = !sc::run_deadline_timed_out(live_suspends, 10_000, 0, deadline);
    //   (W) THE r2 FIX: same latch, but the app stopped polling — heartbeat older than `stale`. Takeover must
    //       no longer suspend, or a hung app strands the shell forever (the wedge the first cut introduced).
    let hung_releases = !sc::takeover_suspends(1, 1, 10_000, 9_000, stale);
    //   (X) a latch naming a DIFFERENT asid never suspends this run; an empty latch never suspends.
    let other_asid_ignored = !sc::takeover_suspends(2, 1, 10_000, 9_990, stale);
    let unlatched_ignored = !sc::takeover_suspends(0, 1, 10_000, 9_990, stale);
    //   (R) once released, the budget re-arms to `now`: not timed out at that instant, timed out a full
    //       deadline later — the liveness bound is genuinely restored, not merely deferred.
    let rearm_start: u64 = 10_000;
    let rearm_fresh = !sc::run_deadline_timed_out(false, rearm_start, rearm_start, deadline); // 0 elapsed
    let rearm_fires = sc::run_deadline_timed_out(false, rearm_start + deadline + 1, rearm_start, deadline);
    // A focus change clears the latch — a fresh `run` always starts disengaged (deadline fully armed).
    sc::user_input_set_active(0); // restore: no active user focus for the real boot
    // EL0IN-FOCUS: last router call is done — hand the cursor keep-alive back to real input.
    ROUTER_SELFTEST.store(false, Ordering::Relaxed);
    let takeover_cleared = sc::el0_takeover_active() == 0;
    // INROUTE: the race window's own witness, printed BEFORE the verdict so a FAIL is always accompanied by
    // its diagnosis. `revokes` counts slot teardowns that revoked the live focus while this test was
    // measuring — the exact event that produced the historical `routed=1` flake. A healthy boot reads
    // `revokes=0` here, because this runs before any user slot exists; anything else means a concurrent owner
    // of the global focus has been reintroduced ahead of this call site and the count below cannot be trusted.
    serial_println!(
        "[inroute] router window — routed={} stale_dropped={} revokes={} gui_sent_delta={}",
        routed,
        stale_dropped as u8,
        sc::el0_focus_revokes().wrapping_sub(revokes0),
        sent1.wrapping_sub(sent0)
    );
    if routed == 2 && sent1 == sent0 {
        serial_println!(
            ":: USER: input router — routed=2 (key+mouse) to active-focus ring, Timer dropped, GUI_CHANNEL bypassed :: PASS ::"
        );
    } else {
        serial_println!(
            ":: USER: input router — routed={} gui_sent_delta={} :: FAIL ::",
            routed,
            sent1.wrapping_sub(sent0)
        );
    }
    if push_does_not_engage
        && stale_dropped
        && live_suspends
        && suspend_holds
        && hung_releases
        && other_asid_ignored
        && unlatched_ignored
        && rearm_fresh
        && rearm_fires
        && takeover_cleared
    {
        serial_println!(
            "[uvug8] takeover deadline — push-does-not-engage, stale-launch-event-dropped, live-suspends, hung-app-releases (liveness bound restored), foreign/empty-latch-ignored, re-arm-fires, clear-on-focus-change :: PASS ::"
        );
    } else {
        serial_println!(
            "[uvug8] takeover deadline — push_no_engage={} stale_dropped={} live_suspends={} suspend_holds={} hung_releases={} other={} unlatched={} rearm_fresh={} rearm_fires={} cleared={} :: FAIL ::",
            push_does_not_engage, stale_dropped, live_suspends, suspend_holds, hung_releases,
            other_asid_ignored, unlatched_ignored, rearm_fresh, rearm_fires, takeover_cleared
        );
    }
}

/// SERIAL-FOCUS QEMU witness — **prove the source split with a focused EL0 window holding the
/// keyboard**, which is the exact state the blocker names.
///
/// HONEST QEMU NOTE, in the shape `input_router_selftest` established for the same reason: the real
/// edge this arc is about — a byte arriving on the PL011 RX FIFO — cannot be produced under
/// `raspi4b`, whose `-serial file:` chardev is write-only. There is nothing to type with. So this
/// drives the pipeline from the seam `input_service` calls (`shell_inbox::offer`, the first line of
/// `serial_to_shell`) rather than from the wire, exactly as the router selftest drives the router
/// from `push_event` rather than from a USB keypress. Everything downstream of that seam — the
/// routing, the bound, the ordering, the accounting — is the production code, unmodified.
///
/// The four legs, and what each one would have shown before this arc:
///
///   (A) FOCUS DOES NOT DIVERT. With `user_input_active()` set to a live-looking ASID — a focused
///       EL0 window owning the keyboard — offer a run of bytes, then run the REAL router drain
///       (`route_input_to_active_el0`, the same call `pump_usb_into_gui` makes). It must route
///       ZERO: serial bytes are not in `EVENT_QUEUE`, so the focus ruling cannot reach them. Before
///       the arc a serial byte that came through `pump_and_poll` WAS in `EVENT_QUEUE` and this
///       would have routed every one of them into the window's ring.
///
///   (B) ORDER, EXACTLY. Drain the inbox and compare against the sequence that was offered, byte
///       for byte. "Ordering within the serial stream is preserved" is a claim about a ring with a
///       wrapping head, so it is checked across a wrap (the run is offered twice, with a drain
///       between, at an offset that puts the second run across the CAP boundary).
///
///   (C) THE STORM IS BOUNDED AND CANNOT JAM THE GUI CHANNEL. Offer `CAP + OVER` bytes with no
///       consumer running. `offer` must return without ever blocking (it returns at all, which is
///       the assertion — the pre-arc `Channel::send` would have parked this task forever at byte
///       65 with no consumer), it must accept exactly `CAP` and refuse exactly `OVER`, the refusals
///       must be accounted in `dropped`, and — the property the brief names — `GUI_SENT` must be
///       UNCHANGED across the whole storm. Not "bounded"; ZERO. The storm does not travel on
///       `GUI_CHANNEL` at all.
///
///   (D) WHAT SURVIVES A STORM IS A PREFIX. Drain the storm-filled ring and confirm the bytes are
///       the FIRST `CAP` offered, in order — the drop-newest policy stated in the `shell_inbox`
///       header. A drop-oldest ring would pass (C) and fail here, and would submit mangled command
///       lines to the shell on a real overflow.
///
/// Runs once on the BSP from the `start_aps` block, before the secondaries exist — the same
/// placement, and for the same ownership reason, as `input_router_selftest` (see INROUTE there).
/// Restores focus 0 and resets the ring's counters on the way out, so the boot's own `[serfocus]`
/// census starts from zero and never inherits this window.
#[cfg(all(target_arch = "aarch64", feature = "baremetal", feature = "witness"))]
fn serial_focus_selftest() {
    use core::sync::atomic::Ordering;
    use unaos_kernel::arch::aarch64::syscall as sc;
    use unaos_kernel::arch::serial::shell_inbox as inbox;

    const RUN: usize = 24;
    const OVER: usize = 37; // deliberately not a divisor of CAP — a wrong bound cannot alias to right
    let sent0 = GUI_SENT.load(Ordering::Relaxed);
    inbox::test_reset();

    // (A) A focused EL0 window owns the keyboard for the whole of this test.
    sc::user_input_set_active(1);
    for i in 0..RUN {
        inbox::offer(b'a'.wrapping_add(i as u8));
    }
    let focus_live = sc::user_input_active() != 0;
    let routed_away = route_input_to_active_el0();
    let held_after_router = inbox::held();

    // (B) Order across a wrap. Drain the first run, then offer a second run positioned so it
    // straddles the CAP boundary, and drain that too.
    let mut order_ok = true;
    for i in 0..RUN {
        order_ok &= inbox::take() == Some(b'a'.wrapping_add(i as u8));
    }
    let drained_clean = inbox::take().is_none();
    // Advance the head to CAP-8 so the next run wraps: offer and drain (CAP - 8 - RUN) bytes.
    let advance = inbox::CAP - 8 - RUN;
    for _ in 0..advance {
        inbox::offer(0);
    }
    for _ in 0..advance {
        inbox::take();
    }
    for i in 0..RUN {
        inbox::offer(b'A'.wrapping_add(i as u8));
    }
    let mut wrap_ok = true;
    for i in 0..RUN {
        wrap_ok &= inbox::take() == Some(b'A'.wrapping_add(i as u8));
    }

    // (C) The storm. No consumer; `offer` must stay total.
    inbox::test_reset();
    let sent_before_storm = GUI_SENT.load(Ordering::Relaxed);
    let mut took = 0usize;
    let mut refused = 0usize;
    for i in 0..(inbox::CAP + OVER) {
        if inbox::offer((i % 251) as u8) {
            took += 1;
        } else {
            refused += 1;
        }
    }
    let storm_gui_delta = GUI_SENT.load(Ordering::Relaxed).wrapping_sub(sent_before_storm);
    let (_, _, dropped, high) = inbox::census();
    let storm_bounded = took == inbox::CAP && refused == OVER && dropped == OVER as u64
        && high == inbox::CAP as u64 && storm_gui_delta == 0;

    // (D) What survived is the FIRST CAP bytes, in order.
    let mut prefix_ok = true;
    for i in 0..inbox::CAP {
        prefix_ok &= inbox::take() == Some((i % 251) as u8);
    }
    let storm_drained_clean = inbox::take().is_none();

    // Restore: no user focus for the real boot, counters zeroed for the live census.
    sc::user_input_set_active(0);
    inbox::test_reset();
    let gui_delta = GUI_SENT.load(Ordering::Relaxed).wrapping_sub(sent0);

    serial_println!(
        "[serfocus] split window — focus_live={} routed_away={} held_after_router={} order={} wrap={} took={} refused={} dropped={} high={} storm_gui_delta={} gui_delta={} cap={}",
        focus_live as u8, routed_away, held_after_router, order_ok as u8, wrap_ok as u8,
        took, refused, dropped, high, storm_gui_delta, gui_delta, inbox::CAP
    );
    if focus_live
        && routed_away == 0
        && held_after_router == RUN
        && order_ok
        && drained_clean
        && wrap_ok
        && storm_bounded
        && prefix_ok
        && storm_drained_clean
        && gui_delta == 0
    {
        serial_println!(
            "[serfocus] split — serial reaches the shell with an EL0 window focused (router routed 0), order preserved across a wrap, storm bounded at cap with GUI_CHANNEL untouched :: PASS ::"
        );
    } else {
        serial_println!(
            "[serfocus] split — focus_live={} routed_away={} held={} order={} clean={} wrap={} storm={} prefix={} storm_clean={} gui_delta={} :: FAIL ::",
            focus_live as u8, routed_away, held_after_router, order_ok as u8, drained_clean as u8,
            wrap_ok as u8, storm_bounded as u8, prefix_ok as u8, storm_drained_clean as u8, gui_delta
        );
    }
}

/// UVUG-6 QEMU witness: prove a held key whose release was DROPPED by a full EVENT_QUEUE can never repeat
/// forever. QEMU raspi4b delivers no HID, so the tracker is driven directly through its public `pal` seams:
///   (A) baseline — a report-level press then a due tick injects the repeat (repeat works at all);
///   (B) backpressure — with EVENT_QUEUE past half full, a due tick REFUSES to inject (the guard that keeps a
///       stuck repeat from saturating the ring and starving real input);
///   (C) dropped-KeyUp — the release NEVER rides the queue; only a report-level release (empty held set) is
///       fed. The tracker disarms, and no repeat is EVER produced across many due ticks — the P51 wedge shut.
/// UVUG-9 adds the fix-DURABILITY legs for the evidence gate that decides which liveness window applies. The
/// gate must latch on a genuine idle re-report and on nothing else, because one false latch re-imposes the 1 s
/// window for the whole boot and brings the ~10-repeat stop straight back:
///   (D) two-key ROLLOVER RELEASE (press a, press b, release a) — no press edge and the armed key still held,
///       which is exactly the shape of an idle re-report except that the held SET shrank. Must NOT latch.
///   (E) NON-ASCII TAP while holding (an F-key maps to ascii 0, so both its press and its release reach the
///       tracker as unchanged-held reports and the byte-identical test alone cannot see them). Must NOT latch,
///       which is what the `IDLE_RUN_TO_LATCH` run threshold buys.
///   (F) genuine idle re-reports — an unchanged held set repeated past the threshold. MUST latch, so the P51
///       wedge guard still arms on the hardware it was written for.
/// PAL-TYPEMATIC adds the CHAIN B legs — the defect KEYSTAT traced and specified but could not fix in its lane
/// (a liveness lapse never re-armed, and the streaming verdict was boot-wide, so a hold could stop dead with
/// the key still down and nothing on the wire):
///   (G) LAPSE THEN STILL-HELD — the lapse disarms, then a report whose held set still contains the key must
///       RE-ARM it and repeat again, with no release and no re-press anywhere in the sequence.
///   (H) LAPSE THEN RELEASED — the same lapse, but the next report's held set is empty. The release outranks
///       the parked key absolutely: nothing re-arms and no repeat is ever produced. This is the leg that
///       proves the re-arm did not reopen the P51 stuck-repeat hole it sits next to.
///   (I) VERDICT SCOPED TO ITS HOLD — latch the streaming verdict legitimately, then end the hold; the verdict
///       must be gone, so the NEXT hold is judged on its own evidence rather than inheriting this one's.
/// Runs once on the BSP before any input/render service task, when EVENT_QUEUE is empty; drains what it pushes.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn typematic_selftest() {
    use unaos_kernel::pal;
    while pal::next_event().is_some() {} // start from an empty ring
    // (A) baseline: press 'x' at the report level, force the repeat due, expect one synthesised 'x'.
    pal::typematic_note_report(b'x', &[b'x']);
    pal::typematic_test_force_due();
    let baseline = pal::typematic_tick() == Some(b'x');
    while pal::next_event().is_some() {}
    // (B) backpressure: still "held"; fill the ring past half full; a due tick must now suppress the inject.
    pal::typematic_note_report(b'x', &[b'x']); // re-arm (baseline's tick advanced NEXT past due)
    for _ in 0..(unaos_kernel::pal::QUEUE_SIZE_PUB / 2 + 4) {
        pal::push_event(pal::Event::Key(b'z'));
    }
    pal::typematic_test_force_due();
    let suppressed = pal::typematic_tick().is_none();
    while pal::next_event().is_some() {}
    // (C) dropped-KeyUp: feed ONLY a report-level release (empty held set) — no KeyUp ever touched the queue.
    pal::typematic_note_report(0, &[]);
    let mut repeated = false;
    for _ in 0..64 {
        pal::typematic_test_force_due();
        if pal::typematic_tick().is_some() {
            repeated = true;
            break;
        }
    }
    while pal::next_event().is_some() {}

    // --- UVUG-9: evidence-gate durability. Each leg starts from a clean tracker. ---
    // (D) rollover release: 'a' down, 'b' down, 'a' released. The final report has no press edge and the armed
    //     key ('b') is still held — an idle re-report's shape — but the held set changed, so it must not latch.
    pal::typematic_test_reset();
    pal::typematic_note_report(b'a', &[b'a']);
    pal::typematic_note_report(b'b', &[b'a', b'b']);
    pal::typematic_note_report(0, &[b'b']); // 'a' released; no press edge; 'b' still armed + held
    let rollover_clean = !pal::typematic_test_streams_latched();

    // (E) non-ascii tap while holding: an F-key contributes no ascii, so its press and release both arrive as
    //     unchanged-held reports. Two of them must stay under the run threshold and must not latch.
    pal::typematic_test_reset();
    pal::typematic_note_report(b'b', &[b'b']);
    pal::typematic_note_report(0, &[b'b']); // F-key down  — invisible in the ascii projection
    pal::typematic_note_report(0, &[b'b']); // F-key up    — likewise
    let nonascii_clean = !pal::typematic_test_streams_latched();

    // (F) genuine idle re-reports: the same held set, repeated past the threshold, MUST latch — otherwise the
    //     P51 wedge guard would never arm on the streaming hardware it exists for.
    pal::typematic_test_reset();
    pal::typematic_note_report(b'b', &[b'b']);
    for _ in 0..(pal::TYPEMATIC_IDLE_RUN_TO_LATCH + 1) {
        pal::typematic_note_report(0, &[b'b']);
    }
    let idle_latches = pal::typematic_test_streams_latched();
    pal::typematic_test_reset();
    while pal::next_event().is_some() {}

    // --- PAL-TYPEMATIC: chain B. The lapse must RE-ARM on evidence, and the verdict must expire with its hold.
    // (G) LAPSE THEN STILL-HELD: a liveness lapse disarms 'g', but the next report still carries 'g' in its
    //     held set. That report is proof the lapse's inference was wrong, so the repeat must resume — WITHOUT
    //     a release and re-press, which is exactly what KEYSTAT recorded as the missing behaviour.
    pal::typematic_test_reset();
    pal::typematic_note_report(b'g', &[b'g']);
    pal::typematic_test_force_lapse();
    let lapse_disarms = pal::typematic_test_armed().is_none();
    pal::typematic_note_report(0, &[b'g']); // no press edge — only "still held"
    let lapse_rearms = pal::typematic_test_armed() == Some(b'g');
    pal::typematic_test_force_due();
    let rearm_repeats = pal::typematic_tick() == Some(b'g');
    while pal::next_event().is_some() {}

    // (H) LAPSE THEN RELEASED: the same lapse, but the next report's held set is EMPTY. A release outranks the
    //     parked key absolutely — nothing may re-arm, and no repeat may EVER be produced afterwards. This is
    //     the leg that keeps the re-arm from reopening the P51 stuck-repeat hole.
    pal::typematic_test_reset();
    pal::typematic_note_report(b'h', &[b'h']);
    pal::typematic_test_force_lapse();
    pal::typematic_note_report(0, &[]); // released
    let release_beats_lapse = pal::typematic_test_armed().is_none();
    let mut ghost_repeat = false;
    for _ in 0..64 {
        pal::typematic_test_force_due();
        if pal::typematic_tick().is_some() {
            ghost_repeat = true;
            break;
        }
    }
    while pal::next_event().is_some() {}

    // (I) VERDICT SCOPED TO ITS HOLD: latch the streaming verdict the legitimate way (leg F's shape), then end
    //     the hold. The verdict must be GONE — boot-wide stickiness is the half of chain B that made a later
    //     silent hold stop after ~15 repeats with nothing on the wire.
    pal::typematic_test_reset();
    pal::typematic_note_report(b'i', &[b'i']);
    for _ in 0..(pal::TYPEMATIC_IDLE_RUN_TO_LATCH + 1) {
        pal::typematic_note_report(0, &[b'i']);
    }
    let hold_latches = pal::typematic_test_streams_latched();
    pal::typematic_note_report(0, &[]); // the hold ends
    let verdict_expires = !pal::typematic_test_streams_latched();
    pal::typematic_test_reset();
    while pal::next_event().is_some() {}

    let chain_b = lapse_disarms
        && lapse_rearms
        && rearm_repeats
        && release_beats_lapse
        && !ghost_repeat
        && hold_latches
        && verdict_expires;

    if baseline && suppressed && !repeated && rollover_clean && nonascii_clean && idle_latches && chain_b {
        serial_println!(
            ":: uvug6: typematic — baseline repeat OK, backpressure suppressed inject, report-level release disarmed dropped-KeyUp hold; UVUG-9 evidence gate: rollover-release + non-ascii-tap did NOT latch, genuine idle re-reports DID; PAL-TYPEMATIC chain B: a liveness lapse RE-ARMS on a still-held report and repeats again, a release outranks it with no ghost repeat, and the streaming verdict expires with its hold :: PASS ::"
        );
    } else {
        serial_println!(
            ":: uvug6: typematic — baseline={} suppressed={} repeated={} rollover_clean={} nonascii_clean={} idle_latches={} lapse_disarms={} lapse_rearms={} rearm_repeats={} release_beats_lapse={} ghost_repeat={} hold_latches={} verdict_expires={} :: FAIL ::",
            baseline,
            suppressed,
            repeated,
            rollover_clean,
            nonascii_clean,
            idle_latches,
            lapse_disarms,
            lapse_rearms,
            rearm_repeats,
            release_beats_lapse,
            ghost_repeat,
            hold_latches,
            verdict_expires
        );
    }
}

/// PIUSB-23 (bare-metal aarch64): pump the xHCI controller from the scheduled input service and bridge
/// decoded HID keys into the GUI channel. THE keyboard-goes-silent structural fix.
///
/// On the Pi baremetal+fb path, `kernel_main` spawns input/render then `hlt_loop`s the BSP, so the
/// x86/virt GUI loop's xHCI hooks (poll_events + service_*) are never reached. After `piusb::enumerate`'s
/// bounded pump returns, nothing consumes re-armed interrupt-IN transfer completions — so `xHCI: KEY`
/// lines stop and USB keystrokes never move. This restores the pump on the input core:
///   (1) `poll_events` drains transfer events; its HID decode (drivers/xhci/mod.rs) prints the
///       `xHCI: KEY` serial witness and pushes each decoded key into `pal::EVENT_QUEUE`;
///   (2) the deferred services run (same set the x86/virt loop runs), keeping enum/hub/HID-setproto work
///       alive; then
///   (3) queued keys are forwarded into `GUI_CHANNEL` in the SAME `Event::Key` shape UART bytes take
///       (see `input_service`), so a USB keystroke reaches the shell/panel exactly like a serial byte.
///
/// The xHCI loan (WEDGE-8) is returned before the drain (its scope ends) so a backpressured
/// `GUI_CHANNEL.send` never blocks while holding the controller. Same global handle `enumerate` seeded.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn pump_usb_into_gui() {
    // WITSWEEP (SERWIT-2 reachability, baremetal leg): the aarch64 baremetal builds never reach the
    // shared BSP loop below (the scheduler path owns them), so the mirror-tap announcement + one-shot
    // verdict ride this pass instead — it runs from `usb_pump` (~4 ms cadence, metal) and from the
    // `input_service` poll-nap branch (every cooperative pass, QEMU raspi4b), i.e. on every baremetal
    // variant. Placed BEFORE the XHCI_CONTROLLER lock: `mirror_service` prints, and its contract is
    // IRQs unmasked, no locks held, non-print context — all true at the top of a scheduled-task pass.
    // A few relaxed atomic loads when quiet; prints only on un-announced loss + the one-shot verdict.
    unaos_kernel::serial_ring::mirror_service();
    // WEDGE-8 (F3): THE F3 HOLDER SITE. This claim is a LOAN, not a lock hold — the services below
    // (service_storage's bring-up matrices reach `pump_until_bot_done`, budget hw_wait_budget()*3
    // ≈ 8.3 s on a failing transfer) run with NO lock held, so a mid-service preemption of this
    // task parks the CONTROLLER, not a lock every masked FS writer spins on. A Busy claim means a
    // block-layer transaction is in flight; skip the pass — the next one is ~4 ms out.
    if let Ok(mut x) = unaos_kernel::drivers::xhci::claim() {
        x.poll_events();
        x.service_storage();
        x.service_hubs();
        x.service_hid_setproto();
        x.service_ftdi();
        x.service_slot_disposal();
        x.service_enum();
    }
    // BOT-CENSUS-PI: the one-shot BOT phase-desync census (`:: BOT: phase … result=SUMMARY ::`) plus
    // the USB topology dump. Its other two call sites ride the shared x86/virt BSP loop, which a Pi
    // bare-metal boot NEVER reaches (the scheduler path owns those boots), so on the platform whose
    // VL805 the counters were written to interrogate the counters incremented and the REPORT never
    // fired — tag_mismatch/bad_sig/undrained/cbw_fault with no denominator. This pass is the Pi's
    // equivalent site: it runs from `usb_pump` (~4 ms cadence, metal) and from the `input_service`
    // poll-nap branch (every cooperative pass, QEMU raspi4b), i.e. on every bare-metal variant.
    //
    // PLACEMENT IS THE WHOLE TRICK: this call sits OUTSIDE the `claim()` loan scope above, after the
    // `if let` block's closing brace has dropped the loan. Since WEDGE-8 `log_summary_once` claims its
    // OWN loan internally, so calling it from inside the block would self-deny with `Busy` — and
    // because the one-shot's N counter advances on EVERY call regardless of whether the claim
    // succeeds, that would burn the single firing and suppress the census for the whole boot,
    // permanently. Nothing between here and the loan's drop may re-claim the controller.
    //
    // Threshold unchanged (N == 2000, deliberately not retuned): at this pass's ~4 ms cadence that is
    // ~8 s after the pump comes up, versus the x86 loop's frame cadence — late enough that boot
    // enumeration has finished or visibly stalled, which is what the one-shot wants either way.
    unaos_kernel::drivers::xhci::log_summary_once();
    // UVUG-5 typematic: BEFORE the drain, synthesise a held key's repeat into EVENT_QUEUE so it rides this
    // pass's routing exactly like a real key edge (`poll_events()` above already re-armed the HID rings and
    // pushed any genuine edges). No key held / not yet due -> no-op; QEMU has no HID so this never fires.
    if let Some(k) = unaos_kernel::pal::typematic_tick() {
        unaos_kernel::pal::push_event(unaos_kernel::pal::Event::Key(k));
    }
    // UVUG-10: producer/consumer accounting for EVENT_QUEUE itself. Placed BEFORE the routing branches so it
    // reports on every pass regardless of which sink owns input this instant — the question it answers ("was
    // a pointer event ever produced at all?") is independent of where events are being routed.
    uvug10_evq_witness();
    // PIUSB-24: bridge decoded KEYS **and** POINTER events (Mouse/MouseAbsolute/Button) into the GUI
    // channel — the render task now moves the shared `pal::cursor` sprite from them (mirroring the x86
    // console loop). The MOUSE-1 witness confirmed relative boot-mouse reports arrive on metal; before
    // this arc the drain forwarded Key alone and silently dropped every pointer report. Timer/None/
    // Unknown are still dropped here (Timer is the render task's own status pulse; None/Unknown carry
    // nothing) so EVENT_QUEUE never accretes on the Pi's channel-based path.
    //
    // GUI-CLICK-2: when a full-screen app owns the screen (SCREEN_APP_ACTIVE — set around
    // dispatch_command in handle_key), do NOT drain EVENT_QUEUE into GUI_CHANNEL. render_service is
    // blocked inside that command and cannot recv, so every forward here would (a) steal the app's
    // input (the app polls the SAME EVENT_QUEUE via pump_and_poll — incl. the exit click) and
    // (b) fill the 64-slot GUI_CHANNEL until `send` blocks this pump task, the metal fps-decay
    // mechanism. Leaving the events untouched hands them to the app's own drain and keeps the
    // channel empty. Normal forwarding resumes the instant the command returns (flag cleared).
    // poll_events() above still ran (it re-arms the HID rings + fills EVENT_QUEUE) — only the
    // forward is suppressed, and the app's pump_and_poll is the sole consumer meanwhile.
    // INPUT-WIRE (ELF-5 router fold): when a user program holds input focus (its ASID registered via
    // `user_input_set_active` in `run_user_image`), route the drained pal events into ITS per-process ring
    // through the `user_input_enqueue` seam — keyboard AND mouse — instead of GUI_CHANNEL. The user app is
    // the SOLE consumer of that ring (it polls via SYS_INPUT_POLL); it cannot reach EVENT_QUEUE, so — unlike
    // the SCREEN_APP_ACTIVE kernel-app gate below, which LEAVES events in EVENT_QUEUE for the app's own
    // `pump_and_poll` drain — we DRAIN here (a left event would never be consumed and would just age out of
    // the 64-slot queue). This check takes PRECEDENCE over SCREEN_APP_ACTIVE: the `run` shell command
    // dispatches through `dispatch_command` (which sets SCREEN_APP_ACTIVE), so both flags are live during an
    // user `run`, and the user ring is the real sink. `poll_events()` above already re-armed the HID rings and
    // filled EVENT_QUEUE; only the destination changes.
    //
    // Watchdog / escape hatch (UVUG-5 correction): the `run` command sets SCREEN_APP_ACTIVE and calls
    // `gui_watchdog::on_app_enter`, arming the 5 s wedge watchdog for the user program too. But a user app
    // drains input through SYS_INPUT_POLL, NOT the kernel `pump_and_poll` that feeds `note_progress` — so the
    // watchdog saw no heartbeat and FALSELY reclaimed a healthy, polling UVUG at 5 s (P47's `[gui] watchdog
    // app wedged 5s`). `sys_input_poll` now calls `note_progress` on every poll (the user twin of the kernel
    // app's per-drain heartbeat), so a live user app is never falsely wedged; a genuinely dead one still loses
    // the screen at the timeout. `run_user_image` clears the focus on return, so the router reverts to the
    // GUI_CHANNEL / SCREEN_APP_ACTIVE paths the instant the program exits — the shell regains the keyboard.
    if unaos_kernel::arch::aarch64::syscall::user_input_active() != 0 {
        route_input_to_active_el0();
        click2_depth_witness();
        wheel1_witness(); // WHEEL — edge-triggered; silent unless a wheel delta actually moved.
        inwedge_witness(); // INWEDGE — edge-triggered; silent unless the panel lock was actually contended.
        return;
    }
    if SCREEN_APP_ACTIVE.load(core::sync::atomic::Ordering::Relaxed) {
        // Non-destructively witness a pending press for the `[click2] input left for app` line, then
        // hand every event back to the app untouched. Drain into a fixed buffer (queue is bounded to
        // 64), scan for a Button, re-push in original order. No heap; the queue is empty for only the
        // few instructions between drain and re-push (the app's next pump_and_poll pass re-reads it).
        //
        // UVUG-10: this peek runs through the UNCOUNTED re-circulation seam
        // (`peek_event_uncounted` / `requeue_event`). Nothing is produced or consumed here — the same
        // events go round again every pass, ~250×/s for as long as a kernel app owns the panel — so
        // counting them would inflate `[uvug10] evq`'s push/pop (and, on a deep ring, drop) precisely in
        // the state where a stalled drain is the hypothesis under test.
        // WC-TAB: the TAB interception lives INSIDE this scan, not below the branch. This gate — not the
        // `user_input_active() != 0` one above — is the gate that actually holds while focus sits in the
        // ring's shell slot, and getting that backwards is what made the first cut of this fix a no-op.
        // `handle_key` sets SCREEN_APP_ACTIVE around `dispatch_command` and it stays set for the WHOLE user
        // run: `run_user_image` parks the shell task in its wait loop until the program returns. So with
        // apps live and focus TAB'd out to the shell, BOTH flags were live, this branch returned first,
        // and the TAB was requeued forever — the exit stayed one-way. (The complement, focus 0 with
        // SCREEN_APP_ACTIVE clear, could not hold two ring entries BEFORE BGRUN-1: a live app implied a
        // parked shell. BGRUN-1 falsifies that — `bg` apps live across the prompt, so focus 0 + flag
        // clear + a full ring is now the NORMAL state; it is handled because this drain calls
        // `wc_shell_focus_key` too, not because the state cannot arise.)
        //
        // It has to be done HERE and not by forwarding the remainder onward: `render_service` is blocked
        // inside the same `dispatch_command`, so anything pushed into the 64-slot GUI_CHANNEL would sit
        // there until `send` blocks this pump task. That saturation is precisely what this branch exists
        // to prevent, so the fix stays within its discipline — peek, consume, requeue — and never sends.
        //
        // A consumed TAB DISCARDS the whole buffer rather than requeuing it. That is not a new policy: it
        // is exactly what `user_input_set_active` would have done to these same events itself, since it
        // drains `pal::EVENT_QUEUE` on every real focus change. They are outside the queue for a few
        // instructions only because this uncounted peek is holding them, and a fresh focus starts clean.
        let mut buf: [unaos_kernel::pal::Event; 64] =
            [unaos_kernel::pal::Event::None; 64];
        let mut n = 0usize;
        let mut saw_button = false;
        let mut cycled = false;
        while n < buf.len() {
            match unaos_kernel::pal::peek_event_uncounted() {
                Some(ev) => {
                    // WC-TAB: `true` from `wc_shell_focus_key` means CONSUMED, which is not the same as
                    // FOCUS MOVED. Its swallow arm also returns true for the RELEASE edge of a Tab whose
                    // press was consumed — no `user_input_set_active`, hence no focus change and no
                    // compensating EVENT_QUEUE drain. That edge arrives on a LATER poll than the press
                    // and can be batched behind real input: TAB out of the ring, then a click, and the
                    // queue is [Button, Mouse, KeyUp(9)]. Treating the swallow as a cycle would drop the
                    // held buffer and suppress the click witness — destroying a mouse click on every TAB
                    // out of the ring, since every such TAB produces this release edge.
                    //
                    // So gate on an ACTUAL transition: snapshot the active ASID and compare. This is the
                    // same test the bare-shell drain below makes before it breaks, which is the symmetry
                    // that was the point of having one shared body.
                    let before = unaos_kernel::arch::aarch64::syscall::user_input_active();
                    if unaos_kernel::arch::aarch64::syscall::wc_shell_focus_key(ev) {
                        if unaos_kernel::arch::aarch64::syscall::user_input_active() != before {
                            cycled = true;
                            break; // focus moved; the next pass routes to the newly-focused app
                        }
                        // Swallow-only: the event is gone (never requeued, so account it), but the
                        // buffer is still the app's and the scan carries on exactly as before.
                        unaos_kernel::pal::note_uncounted_discard(1);
                        continue;
                    }
                    // UVUG-6: typematic is fed at the HID report level, not from this drain.
                    if matches!(ev, unaos_kernel::pal::Event::Button(_)) {
                        saw_button = true;
                    }
                    buf[n] = ev;
                    n += 1;
                }
                None => break,
            }
        }
        if cycled {
            // The buffered events and the consumed TAB leave the pipeline here and are never requeued.
            // They were counted on the way in by `push_event`, so count them out too — otherwise
            // `[uvug10] evq`'s `push - drop - pop` occupancy reads permanently high by `n + 1` per cycle.
            unaos_kernel::pal::note_uncounted_discard(n + 1);
        } else {
            for ev in buf.iter().take(n) {
                unaos_kernel::pal::requeue_event(*ev);
            }
        }
        if saw_button && !cycled {
            click2_left_witness();
        }
        click2_depth_witness();
        return;
    }
    while let Some(ev) = unaos_kernel::pal::next_event() {
        // WC-TAB: the same interception on the bare-shell drain — no user focus AND no screen app. The
        // SCREEN_APP_ACTIVE branch above carries the case that matters for a live ring; this one keeps
        // the two shell paths behaving identically rather than leaving a second, subtly different TAB.
        // `wc_shell_focus_key` shares the in-ring predicate, so TAB is consumed ONLY when there are at
        // least two windows to rotate through; otherwise it falls through untouched and reaches the
        // console exactly as before (`handle_key` ignores byte 9 — there is no completion or other shell
        // binding on TAB to clobber).
        //
        // BREAK, not continue: a consumed TAB has just moved focus, so every remaining event in this
        // drain now belongs to the newly-focused app, not to GUI_CHANNEL. The in-ring path is
        // self-correcting — it re-reads the active ASID per event — but this loop's destination is fixed
        // at `gui_send`, so it would keep posting the new focus's keystrokes to the console. Re-read the
        // active ASID and leave the loop; the next pump pass takes the routing branch.
        // Same transition test as the branch above, written the same way: a consumed TAB may be the
        // swallowed release edge rather than a cycle, and only a real move invalidates this loop's
        // destination. (Events here come off the COUNTED `next_event`, so no discard accounting is owed —
        // unlike the uncounted peek above.)
        let before = unaos_kernel::arch::aarch64::syscall::user_input_active();
        if unaos_kernel::arch::aarch64::syscall::wc_shell_focus_key(ev) {
            if unaos_kernel::arch::aarch64::syscall::user_input_active() != before {
                break;
            }
            continue;
        }
        // UVUG-6: typematic is fed at the HID report level (see pal::typematic_note_report), not this drain.
        match ev {
            unaos_kernel::pal::Event::Key(_) => {
                uvug9_shell_input_witness(true);
                gui_send(ev)
            }
            unaos_kernel::pal::Event::Mouse { x, y } => {
                uvug9_shell_input_witness(false);
                piusb24_pointer_witness(x, y, None);
                gui_send(ev);
            }
            unaos_kernel::pal::Event::MouseAbsolute { x, y } => {
                uvug9_shell_input_witness(false);
                piusb24_pointer_witness(x, y, None);
                gui_send(ev);
            }
            unaos_kernel::pal::Event::Button(mask) => {
                uvug9_shell_input_witness(false);
                piusb24_pointer_witness(0, 0, Some(mask));
                // CLICK-ROUTE — click-to-focus from the SHELL slot. Focus is 0 here, so today every
                // button goes to `gui_send` -> `click1_dispatch`, whose only hit-test is console vs
                // status strip: a click on a live window (a `bg` app's, say) does nothing at all.
                // `wc_click_route` hit-tests the pointer against the window layer; on a hit it raises
                // that window through the ordinary focus primitive, and the press then belongs to the
                // app, not the console. On a MISS it consumes nothing and the console keeps the click.
                //
                // BREAK on a raise, exactly as the TAB interception above breaks and for the same
                // reason: this loop's destination is fixed at `gui_send`, but focus has just moved, so
                // every remaining event belongs to the newly-focused app. The next pump pass takes the
                // `user_input_active() != 0` routing branch and re-reads the target per event.
                //
                // The press itself is delivered HERE rather than left for that next pass because
                // `user_input_set_active` DRAINS `pal::EVENT_QUEUE` as part of granting focus (a fresh
                // focus starts clean — the UVUG-8r2 contract), so an event left behind would not
                // survive the raise. `user_input_enqueue` funnels back through `wc_click_route`, which
                // sees no edge the second time (the mask tracker already advanced) and delivers.
                //
                // POSITION FRESHNESS, stated: on this path the shared cursor is moved by
                // `render_service`'s Mouse arms, one GUI_CHANNEL hop downstream, so the hit-test reads
                // the position as of the last motion report the render task has already consumed. A
                // click is preceded by the pointer coming to rest, so the lag is nil in practice; the
                // user-focus path has no such gap (the router moves the cursor itself — FOCUS-VIS).
                let before = unaos_kernel::arch::aarch64::syscall::user_input_active();
                if unaos_kernel::arch::aarch64::syscall::wc_click_route(ev) {
                    continue;
                }
                if unaos_kernel::arch::aarch64::syscall::user_input_active() != before {
                    unaos_kernel::arch::aarch64::syscall::user_input_enqueue(ev);
                    break;
                }
                gui_send(ev);
            }
            // WHEEL — a scroll that arrived with the SHELL holding focus. There is no EL0 ring to put
            // it in and the console does not scroll, so the event ends here; what this arm adds is the
            // ACCOUNTING, so the census can say "decoded, but nobody was listening" instead of leaving
            // a working decoder and a silent machine looking identical. Deliberately NOT forwarded to
            // `gui_send`: the render service has no wheel consumer, and inventing one would be the
            // scroll feature this arc is explicitly not landing.
            unaos_kernel::pal::Event::Wheel(_) => {
                unaos_kernel::pal::wheel_note_no_focus();
            }
            _ => {}
        }
    }
    click2_depth_witness();
    wheel1_witness(); // WHEEL — edge-triggered; silent unless a wheel delta actually moved.
    inwedge_witness(); // INWEDGE — edge-triggered; silent unless the panel lock was actually contended.
    // PIUSB-28: fire the FAT mount from the path that ACTUALLY runs on Pi baremetal+fb. PIUSB-27
    // wired `piusb27_service()` beside `probe_once` in the main/GUI loop — but that loop never runs
    // on Pi metal (kernel_main spawns services and hlt_loops; the PIUSB-22 structural finding, which
    // is why `usb_pump` exists), so the mount never fired on hardware (zero `piusb27` lines, P35).
    // This pump path runs on metal (the `usb_pump` ~4 ms task) and in QEMU raspi4b (the input
    // service's poll-nap fallback), so the storage-ready edge is finally polled where it matters.
    //
    // LOAN DISCIPLINE (was DEADLOCK): the mount re-claims the xHCI loan via `read_block_usb`, so it
    // MUST run outside any scope holding the loan — a claim while we still held it would return
    // Busy and the mount would fail for the pass. The loan above is returned at the end of its
    // `if let` scope (before the event-drain loop), so we are loan-free here. The edge is one-shot
    // per raise (`take_usb_ready`), so calling it every ~4 ms pass is a cheap no-op until a stick's
    // bring-up raises it, then it mounts + witnesses once (and again on every hot-plug re-enum).
    if !PIUSB28_ARMED.swap(true, core::sync::atomic::Ordering::Relaxed) {
        serial_println!(":: piusb28: mount-trigger armed (pi pump path) ::");
    }
    unaos_kernel::fs::fat::piusb27_service();
}

/// PIUSB-24: rate-limited (~4 Hz) `[piusb24]` serial witness of a pointer report reaching the GUI
/// bridge — dx/dy for motion (relative or absolute payload) or the button bitmask for a click edge.
/// A moving mouse emits reports far faster than serial should mirror, so log at most every ~250 ms;
/// button edges are rare and always logged (they bypass the throttle via the `buttons` arm).
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn piusb24_pointer_witness(dx: i32, dy: i32, buttons: Option<u8>) {
    use core::sync::atomic::Ordering;
    if let Some(mask) = buttons {
        serial_println!("[piusb24] pointer buttons=0b{:08b}", mask);
        return;
    }
    let now = unaos_kernel::arch::ms();
    let last = PIUSB24_LAST_LOG_MS.load(Ordering::Relaxed);
    if now.wrapping_sub(last) >= 250 || last == 0 {
        PIUSB24_LAST_LOG_MS.store(now.max(1), Ordering::Relaxed);
        serial_println!("[piusb24] pointer dx={} dy={}", dx, dy);
    }
}

/// UVUG-9 — the SHELL-PATH input bisect for the dead-mouse-after-timeout symptom (P54b metal fact 2: once a
/// UVUG run timed out, arrow keys still reached the shell but the mouse produced no cursor and no effect, for
/// the rest of the boot).
///
/// The pointer's journey to the shell has four stages: xHCI decodes the interrupt-IN report and pushes a
/// `pal::Event`; the router drains `EVENT_QUEUE`; `gui_send` forwards into `GUI_CHANNEL`; `render_service`
/// moves the cursor sprite. Existing witnesses bracket the ends — `MOUSE-1` counts reports at the xHCI decode,
/// `[click2] depth` counts `GUI_CHANNEL` traffic — but neither separates "the pointer stopped being decoded"
/// from "the pointer is decoded but no longer reaches the shell path", and those two have completely different
/// owners. This line closes that gap by counting keys and pointer events SEPARATELY at the shell-destined
/// drain, which is precisely the branch that resumes when `run_user_image` drops focus.
///
/// Reading it at P55, with the mouse dead at the shell:
///   * `MOUSE-1` still counting while `ptr=` here is frozen -> the loss is between the decode and this drain
///     (the user ring / router / EVENT_QUEUE seam) — this lane.
///   * `MOUSE-1` ALSO frozen while `key=` here keeps advancing -> the pointer interrupt-IN endpoint stopped
///     completing or stopped being re-armed, and the loss is upstream of every input path in this file. Note
///     that `drivers::xhci`'s dup-Success guard (`mouse_expect_phys`) returns from the transfer dispatch
///     WITHOUT calling `queue_mouse_read`, so a single mismatched completion retires the pointer read forever
///     while the keyboard's independently-armed endpoint carries on — the exact asymmetry P54b describes, on
///     the endpoint that generates by far the most traffic. That file is outside this arc's lane, so this arc
///     instruments the question rather than changing the driver; P55's reading of these two counters decides it.
/// Rate-limited to ~2 s and printed only when something actually arrived, so an idle shell stays silent.
///
/// SERIAL-FOCUS — SCOPE NOTE, so a capture is never over-read. This line speaks for the FOCUS-ROUTED
/// carrier and for nothing else: `EVENT_QUEUE -> GUI_CHANNEL` is the USB HID path, and its `key=`
/// counter has never included, and now structurally cannot include, a byte from the serial console.
/// Serial is consumed ahead of the routing decision this drain implements and delivered on its own
/// carrier; `[serfocus]` is its census. A boot where the operator drives the box over the wire will
/// therefore show `[serfocus] delivered=` climbing with this line's `key=` flat, and that pairing is
/// the arc working, not a keyboard that stopped.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn uvug9_shell_input_witness(is_key: bool) {
    use core::sync::atomic::Ordering;
    let n = if is_key {
        UVUG9_SHELL_KEYS.fetch_add(1, Ordering::Relaxed) + 1
    } else {
        UVUG9_SHELL_PTRS.fetch_add(1, Ordering::Relaxed) + 1
    };
    let _ = n;
    let now = unaos_kernel::arch::ms();
    let last = UVUG9_SHELL_LAST_MS.load(Ordering::Relaxed);
    if now.wrapping_sub(last) >= 2000 || last == 0 {
        UVUG9_SHELL_LAST_MS.store(now.max(1), Ordering::Relaxed);
        serial_println!(
            "[uvug9] shell-path input key={} ptr={} (EVENT_QUEUE -> GUI_CHANNEL)",
            UVUG9_SHELL_KEYS.load(Ordering::Relaxed),
            UVUG9_SHELL_PTRS.load(Ordering::Relaxed)
        );
    }
}

/// SERIAL-FOCUS — ms() of the last `[serfocus]` census line, rate-limiting it to ~2 Hz. 0 = never.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static SERFOCUS_LAST_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// SERIAL-FOCUS — latched once the FIRST serial byte has been delivered to the shell, so the boot
/// states the fact once, with the focus that was live at that instant, rather than only in a rollup.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static SERFOCUS_FIRST: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// SERIAL-FOCUS — **THE CENSUS, and the line the next bench sitting reads.**
///
/// `[uvug9] shell-path input … (EVENT_QUEUE -> GUI_CHANNEL)` above is the witness that NAMED this
/// blocker, and it can only ever speak for the focus-routed carrier. It has no vocabulary for the
/// serial console, because before this arc the serial console had no carrier of its own — its bytes
/// were `Event::Key`s indistinguishable from HID by the time anything counted them. This is that
/// missing half: the same question ("did input reach the shell, and how much") asked of the SOURCE
/// the `[uvug9]` line cannot see.
///
/// `accepted` is what the ring took off the wire, `delivered` is what the shell's key path actually
/// consumed, `dropped` is what a full ring refused, and `high` is the ring's watermark. The
/// invariant a capture can check without any other line: `accepted == delivered + held`, always. A
/// non-zero `dropped` is a capacity reading and not a fault — it says the shell was parked for more
/// than `CAP` bytes of traffic — but it is never silent, which is the entire point of counting it.
/// `focus` is `user_input_active()` AT THE MOMENT OF DELIVERY, and it is the field that proves the
/// arc: a line reading `focus=<non-zero>` with `delivered` climbing IS the claim "a focused EL0
/// window holds the keyboard and the serial wire still drives the shell", stated on the wire.
///
/// DEFAULT-QUIET, and why this family is NOT `witness`-gated when so many are. The law's concern is
/// instruments people learn to ignore, and the test it applies is whether a family speaks on boots
/// that are not asking it anything. This one is silent on every such boot BY CONSTRUCTION: it prints
/// only when a serial byte is actually delivered to the shell, so it emits exactly zero lines on
/// every gate in the battery (QEMU raspi4b's `-serial file:` chardev is write-only — nothing can be
/// typed) and zero lines on any metal boot nobody is typing on. Gating it on `witness` would instead
/// mean the ONE configuration that can never produce it is the only one that carries the strings,
/// and the flashable `./arroyo kernel8` image an attended bench actually boots would be mute for the
/// arc written to unblock that bench. The QEMU verdict for this arc is carried by `[serfocus] split`
/// (the fixture), which IS `witness`-gated in the ordinary way.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn serfocus_witness() {
    use core::sync::atomic::Ordering;
    let (accepted, delivered, dropped, high) =
        unaos_kernel::arch::serial::shell_inbox::census();
    let focus = unaos_kernel::arch::aarch64::syscall::user_input_active();
    if !SERFOCUS_FIRST.swap(true, Ordering::Relaxed) {
        serial_println!(
            "[serfocus] first serial byte delivered to the shell — focus={} app={} (SERIAL -> SHELL INBOX, focus not consulted) == witness ::",
            focus,
            SCREEN_APP_ACTIVE.load(Ordering::Relaxed) as u8
        );
    }
    let now = unaos_kernel::arch::ms();
    let last = SERFOCUS_LAST_MS.load(Ordering::Relaxed);
    if now.wrapping_sub(last) >= 500 || last == 0 {
        SERFOCUS_LAST_MS.store(now.max(1), Ordering::Relaxed);
        serial_println!(
            "[serfocus] serial-in accepted={} delivered={} dropped={} held={} high={} cap={} focus={} app={}",
            accepted,
            delivered,
            dropped,
            unaos_kernel::arch::serial::shell_inbox::held(),
            high,
            unaos_kernel::arch::serial::shell_inbox::CAP,
            focus,
            SCREEN_APP_ACTIVE.load(Ordering::Relaxed) as u8
        );
    }
}

/// UVUG-10 — the PRODUCER-side half of the pointer bisect, and the line that decides ownership of the P55b
/// dead-mouse symptom in a single boot.
///
/// `[uvug9] shell-path` (below) counts pointer events at the router DRAIN and read `ptr=0` on metal forever,
/// from boot, while the xHCI `MOUSE-1` witness reported a live pointer WITH REAL DELTAS. Those two witnesses
/// bracket `pal::EVENT_QUEUE` without measuring it. The upstream half is now settled: the driver's
/// `push_event(Event::Mouse)` precedes the `MOUSE-1` print in straight-line code with no fork between them,
/// so `last dx=3 dy=5` proves the pointer events were pushed — **the loss is at or after the queue**.
///
/// The leading theory is the one this arc's fixture gate already kills: the boot `input_launcher` orphan
/// held `user_input_active()` for the whole boot, so `route_input_to_active_el0` (above) swallowed the queue
/// into a ring nothing would read, while keys still reached the shell through `input_service`'s direct
/// `gui_send` — a path that bypasses EVENT_QUEUE entirely. This witness is what proves or refutes that on
/// the wire instead of by argument.
///
/// Rate-limited by a PASS COUNTER, not wall-clock, for the same reason `[click2] depth` is: this pump also
/// runs from the raspi4b poll-nap fallback where `arch::ms()` is pinned at 0 and a wall-clock throttle never
/// holds. Printed only when a counter actually MOVED since the last line, so an idle machine (and the QEMU
/// battery, where the only producers are the one-shot selftests) stays quiet after the first report instead
/// of adding a periodic line to every log.
///
/// BASELINE: the boot selftests produce events too — `input_router_selftest` alone pushes a synthetic
/// `Mouse{3,-4}`. "Never produced" therefore reads `push ptr=1`, not `push ptr=0` (QEMU's own line is
/// `push ptr=1 key=38 / drop ptr=0 key=0 / pop=40`). Re-circulated events from the `SCREEN_APP_ACTIVE` peek
/// are excluded at the source (`pal::requeue_event`), so these totals never inflate while an app owns the
/// panel.
///
/// P56 verdict table, read against the `[uvug9]` line (fixture now gated off metal, so no orphan should
/// exist and `user_input_active()` should be 0 all boot):
///   * EXPECTED: `[uvug9] ptr` climbs with a moving mouse, `push ptr` climbs with it -> the orphan theory
///     held and the fixture gate was also the mouse fix.
///   * `[uvug9] ptr=0` STILL, no orphan alive -> orphan theory refuted, hunt resumes at/after the queue:
///       - `push ptr>1`, `drop ptr≈push ptr` -> saturated ring behind a stalled drain (cross-read `depth`
///         and `[click2] depth`).
///       - `push ptr>1`, `drop ptr=0` -> a SECOND consumer takes them before the shell drain; `pop` far
///         above the router's own `[uvug9]` totals names it (a user focus ring, or the focus-change
///         pre-launch discard).
///       - `push ptr=1` (selftest floor, unmoved) -> should be unreachable given the settled xHCI finding;
///         if seen, the pointer endpoint stopped completing and the question returns to the driver lane.
///         Confirm against `MOUSE-1`'s report count before concluding that.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn uvug10_evq_witness() {
    use core::sync::atomic::Ordering;
    let pass = UVUG10_EVQ_PASSES.fetch_add(1, Ordering::Relaxed);
    if pass % UVUG10_EVQ_EVERY != 0 {
        return;
    }
    let (push_ptr, push_key, drop_ptr, drop_key, pop) = unaos_kernel::pal::event_queue_stats();
    // Fold the five totals into one value purely to detect "nothing moved" cheaply; the sum can alias in
    // principle, but only across a window in which the counters changed by exactly offsetting amounts —
    // impossible here, since every counter is monotonically increasing.
    let fold = push_ptr
        .wrapping_add(push_key)
        .wrapping_add(drop_ptr)
        .wrapping_add(drop_key)
        .wrapping_add(pop);
    if fold == UVUG10_EVQ_LAST_FOLD.swap(fold, Ordering::Relaxed) {
        return; // nothing was produced or consumed since the last line — stay silent
    }
    serial_println!(
        "[uvug10] evq push ptr={} key={} / drop ptr={} key={} / pop={} depth={}",
        push_ptr,
        push_key,
        drop_ptr,
        drop_key,
        pop,
        unaos_kernel::pal::event_queue_depth()
    );
}

/// UVUG-10 `[uvug10] evq` pass throttle (~5 s at the metal ~250 Hz pump cadence) + the last-reported fold,
/// so an unchanged snapshot is not reprinted.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static UVUG10_EVQ_PASSES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
const UVUG10_EVQ_EVERY: u64 = 1250;
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static UVUG10_EVQ_LAST_FOLD: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// UVUG-9 shell-path input counters + throttle (see `uvug9_shell_input_witness`).
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static UVUG9_SHELL_KEYS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static UVUG9_SHELL_PTRS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
static UVUG9_SHELL_LAST_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// GUI-CLICK-2: rate-limited (~5 s) `[click2] input left for app` witness — proves a press edge
/// arrived while a full-screen app owned the screen and was LEFT in EVENT_QUEUE for the app's own
/// pump_and_poll (rather than forwarded into the render task's channel). The click that exits vug
/// rides this path; the throttle keeps a held/chattering button from flooding serial.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn click2_left_witness() {
    use core::sync::atomic::Ordering;
    let now = unaos_kernel::arch::ms();
    let last = CLICK2_LEFT_LAST_MS.load(Ordering::Relaxed);
    if now.wrapping_sub(last) >= 5000 || last == 0 {
        CLICK2_LEFT_LAST_MS.store(now.max(1), Ordering::Relaxed);
        serial_println!("[click2] input left for app");
    }
}

/// GUI-CLICK-2 (scope addition): rate-limited (~5 s) `[click2] depth` witness — the live GUI_CHANNEL
/// queue depth (`GUI_SENT - GUI_RECV`) plus lifetime forwarded/received totals, so the metal serial
/// carries the channel-saturation curve directly. Printed ALWAYS (both app-active and idle passes),
/// so the fps-decay-under-vug mechanism is proven rather than inferred: depth spiking toward the
/// 64-slot cap while an app runs is the backlog the SCREEN_APP_ACTIVE gate now prevents. (Exact
/// `pal::EVENT_QUEUE` depth and heap-free would need accessors in pal.rs / allocator.rs — outside
/// this arc's lane — so they are omitted here; GUI_CHANNEL depth is the direct suspect.)
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn click2_depth_witness() {
    use core::sync::atomic::Ordering;
    // Fire on pass 0, then once per CLICK2_DEPTH_EVERY passes — timer-independent (see the static's
    // doc: raspi4b's stuck ms() would defeat a wall-clock throttle).
    let pass = CLICK2_DEPTH_PASSES.fetch_add(1, Ordering::Relaxed);
    if pass % CLICK2_DEPTH_EVERY == 0 {
        let sent = GUI_SENT.load(Ordering::Relaxed);
        let recv = GUI_RECV.load(Ordering::Relaxed);
        let app = SCREEN_APP_ACTIVE.load(Ordering::Relaxed);
        serial_println!(
            "[click2] depth gui_chan={} (sent={} recv={}) app_active={}",
            sent.wrapping_sub(recv), sent, recv, app
        );
    }
}

/// WHEEL — the `[wheel1]` census line: `decoded` nonzero wheel deltas, `routed` into a focused EL0
/// app's ring, `nofocus` dropped because nothing was focused, and the sign of the last delta.
///
/// EDGE-TRIGGERED, not periodic, and that is the whole design. `[click2]` fires every N passes because
/// queue depth is a level that is interesting even when it is not moving; a wheel census is not — it
/// is all zeros on every boot that never touched a wheel, which is every automated gate and most
/// sittings. A periodic print would therefore add a line to the knob-off serial log for a subsystem
/// that did nothing, and this track's regression suite counts lines. So the line fires ONLY when the
/// census has actually advanced since the last print, which means: on a boot with no wheel input it is
/// never emitted at all, and on a boot with wheel input it is emitted for every change and no more.
/// Reachability on a gate that cannot deliver a wheel is therefore proved by `strings` on the image,
/// not by the log — stated plainly rather than dressed up as a behavioural pass.
///
/// Called from BOTH pump destinations — the focused-app router branch and the shell drain — because
/// the two answer different halves of the question: the router branch is where a wheel can be routed,
/// and the shell branch is where a wheel arrives when there is nobody to route it to.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn wheel1_witness() {
    use core::sync::atomic::Ordering;
    /// The `decoded + routed + nofocus` sum at the last print. A sum is sufficient as the change
    /// detector: all three counters are monotonic, so the sum advances iff any one of them did.
    static LAST_TOTAL: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let (decoded, routed, nofocus, last) = unaos_kernel::pal::wheel_census();
    let total = decoded.wrapping_add(routed).wrapping_add(nofocus);
    if total != LAST_TOTAL.swap(total, Ordering::Relaxed) {
        serial_println!(
            "[wheel1] decoded={} routed={} nofocus={} last={}",
            decoded, routed, nofocus, last
        );
    }
}

/// INWEDGE — the `[inwedge]` recurrence witness: how many of the input router's panel-geometry reads
/// completed, and how many were REFUSED because `video::WRITER` was held elsewhere at that instant.
///
/// A refusal is the boot-8 window, entered and survived. Before this arc that same instant was a
/// blocking acquire from a preemptible task on the input core, and the capture shows what it cost: the
/// lock left held across a dispatch, then the IRQ-context printer of the very next timer tick spinning
/// on it forever with interrupts masked, taking `input`, `usb-pump`, `rx-backstop` and `status-tick`
/// down together. So a nonzero `refused` is not a fault line — it is the ONE thing that says the race
/// is real on this hardware and that the machine walked away from it. Zero says the window was never
/// even entered on this boot, which is the common case and is why the line is EDGE-TRIGGERED: it is
/// silent on every boot that never contended, including every automated gate, so it adds no line to
/// the knob-off serial log for a subsystem that did nothing.
///
/// Called from the same two pump destinations as `[wheel1]`, for the same reason.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn inwedge_witness() {
    use core::sync::atomic::Ordering;
    /// The refusal count at the last print. Only REFUSALS gate the line: `read` advances on every
    /// pointer report of a live sitting, and a line per report would be a flood.
    static LAST_REFUSED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let (read, refused) = inwedge_census();
    if refused != LAST_REFUSED.swap(refused, Ordering::Relaxed) {
        serial_println!(
            "[inwedge] panel-lock read={} refused={} — the input router declined a held WRITER instead of blocking on it (boot-8 wedge window, survived)",
            read, refused
        );
    }
}

/// INWEDGE — the QEMU-reachable proof. Holds `video::WRITER` and then drives the input router's OWN
/// panel-geometry read from the SAME core, asserting it comes back REFUSED (clamped to 0) instead of
/// blocking — and that it comes back with the real geometry once the lock is released.
///
/// WHY THIS IS A REAL GATE AND NOT A TAUTOLOGY. At the base sha this leg does not fail, it HANGS: the
/// old `pal_width_hint` was `WRITER.lock()`, a blocking acquire on a lock this function is holding, on
/// one core. That hang IS the boot-8 wedge, reproduced deterministically on a QEMU that can deliver no
/// HID at all — the metal capture needed a pointer report and a timer tick to line up, this needs
/// neither. Runs once on the BSP, before the input/render tasks are spawned, so nothing else can be
/// touching `WRITER` and the census it reads is its own.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn inwedge_selftest() {
    /// LOCKFIX — released-half attempts. The old assertion was `read2 == read0 + 1` off a SINGLE
    /// released read, which is a race against the live APs rather than a statement about this
    /// kernel: any secondary holding `WRITER` for its own present at that instant makes our
    /// non-blocking read refuse (`free_w == 0`, `read2 == read0`) and the leg prints FAIL on a
    /// correct kernel. The read is idempotent and costs a try_lock, so retry it — a kernel whose
    /// released read genuinely cannot succeed will exhaust all 64 and still convict.
    const RELEASED_TRIES: u32 = 64;
    let (read0, refused0) = inwedge_census();
    let (refused_w, refused_h) = {
        let _held = unaos_kernel::video::WRITER.lock(); // the holder the router must not block on
        (pal_width_hint(), pal_height_hint())
    };
    let (_, refused1) = inwedge_census();
    // Taken BEFORE the released half so the retry below has something to compare against, and with a
    // blocking acquire because this core is unmasked and holds nothing — the one place in this leg
    // where waiting is legal, and the statement of what the released read must reproduce.
    let real = unaos_kernel::video::WRITER.lock().info().width as i32;
    let mut free_w = 0;
    let mut read2 = read0;
    let mut tries = 0;
    while tries < RELEASED_TRIES {
        tries += 1;
        free_w = pal_width_hint();
        read2 = inwedge_census().0;
        if free_w == real && read2 > read0 {
            break;
        }
    }
    // The HELD half keeps its exact go-red: BOTH reads taken against a lock this core holds must
    // come back clamped to 0 AND counted as refusals. `>=` on the refusal delta only tolerates an AP
    // refusing alongside us; it cannot excuse a read of ours that succeeded, because a successful
    // read would have returned the real geometry into `refused_w`/`refused_h` and failed `== 0`.
    let declined = refused_w == 0 && refused_h == 0 && refused1 >= refused0 + 2;
    let recovered = free_w == real && read2 > read0;
    serial_println!(
        ":: INWEDGE: router panel-lock — held: w={} h={} refused+{} | released: w={} real={} read+{} tries={} :: {} ::",
        refused_w, refused_h, refused1 - refused0, free_w, real, read2 - read0, tries,
        if declined && recovered { "PASS" } else { "FAIL" }
    );
}

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
            // SERIAL-FOCUS — THE SITE. This loop used to be `gui_send(Event::Key(byte))`, which is
            // where the serial wire lost every argument with GUI focus: a blocking `send` into a
            // channel whose consumer parks for the whole life of a foreground command. Now the byte
            // goes to the shell's own inbox and the send, if it happens at all, is one coalesced
            // wake token. Draining the FIFO to completion is unchanged and is now unconditionally
            // safe — `serial_to_shell` has no blocking edge, so this loop can never stall holding a
            // re-arm that the ISR is waiting on.
            while let Some(byte) = unaos_kernel::arch::poll_input() {
                serial_to_shell(byte);
            }
            serial::rearm_rx_interrupt(); // re-enable IMSC (no ICR — keeps a straggler's timeout)
            // Close the drain/re-arm gap: if a byte landed meanwhile, wake ourselves to drain it
            // rather than wait for the next receive-timeout.
            if serial::rx_pending() {
                serial::RX_READY.post();
            }
            // PIUSB-26: the xHCI pump no longer rides this UART wake. PIUSB-23 pumped here, but the
            // metal wake cadence is the RX ISR (keystrokes) or the ~5 Hz rx-backstop poke — so pointer
            // reports batched to ~5 fps ("very very slow", P33). The dedicated `usb_pump` task now
            // drains the controller at ~4 ms, leaving this keyboard/UART interrupt path untouched.
        }
    } else {
        // Poll-nap fallback (QEMU raspi4b: the RX ISR never fires). Cooperative — the AP's run() keeps
        // re-dispatching us; sleep_ticks would park forever with no timer IRQ to wake it.
        //
        // PULSE-STRIP: this branch also carries the strip's 1 Hz refresh pulse. On metal that pulse is
        // the `status-tick` task, which is timer-gated and therefore NOT spawned here — so before this
        // arc the status strip only ever refreshed in QEMU when a key arrived, and an always-running
        // pulse would have been a frozen picture under the gate. Riding the existing poll-nap costs no
        // task (KILLBOUND bounds the table at 8) and no timer: a wall-clock compare per cooperative
        // pass. Gated on SCREEN_APP_ACTIVE for the same reason `status_tick` is — while a full-screen
        // app owns the panel the render task is blocked inside dispatch_command and cannot drain
        // GUI_CHANNEL, so an ungated 1 Hz post would slowly fill the 64-slot channel.
        let mut strip_pulse_ms = unaos_kernel::arch::ms();
        loop {
            // SERIAL-FOCUS — the poll-nap twin of the interrupt branch's site above, same seam and
            // same reasoning. (QEMU raspi4b's `-serial file:` chardev is write-only, so `poll_input`
            // returns `None` on every pass here and this loop is inert for the whole gate battery —
            // which is why the QEMU proof of this arc is the `[serfocus]` fixture, driven through
            // the same `shell_inbox` seam, and not the wire.)
            while let Some(byte) = unaos_kernel::arch::poll_input() {
                serial_to_shell(byte);
            }
            let now = unaos_kernel::arch::ms();
            if now.wrapping_sub(strip_pulse_ms) >= unaos_kernel::ui_status::PSTRIP_PERIOD_MS {
                strip_pulse_ms = now;
                if !SCREEN_APP_ACTIVE.load(Ordering::Relaxed) {
                    gui_send(unaos_kernel::pal::Event::Timer);
                }
            }
            // PIUSB-23: pump xHCI + bridge decoded HID keys into GUI_CHANNEL each cooperative pass
            // (QEMU raspi4b delivers no USB HID, so this is a cheap no-op there; on metal it consumes
            // the re-armed interrupt-IN completions the enumerate() pump no longer services).
            pump_usb_into_gui();
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
    // SPIN-4: phase markers — every prior theory about WHERE this task stalls (semaphore lock,
    // run-queue lock, LL/SC false sharing) died with clean witnesses while the stall persisted.
    // The task now states its own position: [spin1] prints phase+loops, so the stalled call names
    // itself. 1 = about to sleep, 2 = returned from sleep / entering post.
    use unaos_kernel::arch::sched::{RX_BS_LOOPS, RX_BS_PHASE};
    use core::sync::atomic::Ordering;
    loop {
        RX_BS_PHASE.store(1, Ordering::Relaxed);
        unaos_kernel::arch::sched::sleep_ticks(50); // ~200 ms at the 250 Hz per-core tick
        RX_BS_PHASE.store(2, Ordering::Relaxed);
        unaos_kernel::arch::serial::RX_READY.post();
        RX_BS_LOOPS.fetch_add(1, Ordering::Relaxed);
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
/// GUI-CLICK-1: the view a screen point falls in, in the *shared* GUI model. The Pi/x86 panel is a
/// full-screen text console with the always-on status strip (`ui_status`) pinned to the bottom
/// line-pitch band — there are no windows/buttons/close-boxes in the shared model, so those are the
/// only two hit regions. `None` is impossible for an in-bounds point today (the two regions tile the
/// panel) but is kept so the witness can name a miss if the model later grows insets.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
#[derive(Clone, Copy)]
enum Click1Hit {
    /// The bottom status strip (host / lease IP / wall clock) — `ui_status`'s one-line band.
    Status,
    /// The console / shell text area — the focused interactive view.
    Console,
}

#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
impl Click1Hit {
    fn name(self) -> &'static str {
        match self {
            Click1Hit::Status => "status",
            Click1Hit::Console => "console",
        }
    }
}

/// GUI-CLICK-1: hit-test a cursor position against the shared GUI model. Mirrors `ui_status::draw`'s
/// geometry exactly (`band_y = height - line_h`) so the strip's drawn band and its click target can
/// never disagree. A point on or below `band_y` hits the status strip; everything above it is the
/// console. Read-only — takes the same public `metrics()`/`height()` the strip draw uses.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn click1_hit_test(y: i32, pal: &unaos_kernel::pal::TargetPal<'_>) -> Option<Click1Hit> {
    use unaos_kernel::pal::GneissPal;
    let h = pal.height() as i32;
    let line_h = pal.metrics().line_h as i32;
    let band_y = h.saturating_sub(line_h);
    if y < 0 || y >= h {
        None
    } else if y >= band_y {
        Some(Click1Hit::Status)
    } else {
        Some(Click1Hit::Console)
    }
}

/// GUI-CLICK-1: rate-limited (~10 Hz) `[click1]` serial witness of a click reaching GUI dispatch —
/// the cursor position, the button bitmask that produced the press edge, and the hit target's name
/// (or `none` for a miss). Rare vs motion, but throttled so a chattering switch can't flood serial.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn click1_witness(x: i32, y: i32, mask: u8, hit: Option<Click1Hit>) {
    use core::sync::atomic::Ordering;
    let now = unaos_kernel::arch::ms();
    let last = CLICK1_LAST_LOG_MS.load(Ordering::Relaxed);
    if now.wrapping_sub(last) >= 100 || last == 0 {
        CLICK1_LAST_LOG_MS.store(now.max(1), Ordering::Relaxed);
        let target = hit.map(Click1Hit::name).unwrap_or("none");
        serial_println!(
            "[click1] x={} y={} btn=0b{:08b} hit={}",
            x, y, mask, target
        );
    }
}

/// GUI-CLICK-1: dispatch a pointer-button report to the shared GUI model. Called from the render
/// task's `Button` arm with the current sprite position (`pal::cursor::pos`). Acts on a PRESS edge
/// only — a bit that went 0→1 since the last report (`CLICK1_PREV_MASK`) — so a press+release fires
/// once and a held drag doesn't re-fire. On a press it hit-tests the cursor and delivers a click to
/// the hit view: the console is the focused interactive view, so a click on it reasserts the shell's
/// input line (focus/redraw — the same non-destructive activation the shared model already exposes,
/// mirroring vug's "a click is a keystroke-equivalent activation of the focused view"); a click on
/// the status strip only witnesses (it draws nothing interactive). Returns whether a repaint of the
/// console is owed. Never touches the key or motion paths.
///
/// SHELLWIN-PI — `backdrop_console` is the caller's answer to "is the shell still the desktop layer?".
/// The console-activation redraw below writes the prompt at PANEL coordinates, so once the shell is a
/// compositor window that redraw would paint shell glyphs straight back onto the scene the arc just
/// took them off — the SHELLNOTDESK regression, re-entered through the pointer instead of the keyboard.
/// On the desktop the hit-test and the `[click1]` witness still run (the wire keeps saying where the
/// click landed); only the backdrop write is withheld, and it is withheld because there is nothing
/// there to activate: a press that lands ON the shell window never reaches this function at all, being
/// consumed upstream by `wc_click_route`'s kernel-furniture arm.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn click1_dispatch(
    mask: u8,
    console: &unaos_kernel::console::Console,
    pal: &mut unaos_kernel::pal::TargetPal<'_>,
    backdrop_console: bool,
) {
    use core::sync::atomic::Ordering;
    use unaos_kernel::pal::GneissPal;
    let prev = CLICK1_PREV_MASK.swap(mask, Ordering::Relaxed);
    // Newly-pressed bits: set now, clear before. No new press → nothing to dispatch (this is the
    // release edge, or an unchanged held state).
    let pressed = mask & !prev;
    if pressed == 0 {
        return;
    }
    let (x, y) = unaos_kernel::pal::cursor::pos(pal.width() as i32, pal.height() as i32);
    let hit = click1_hit_test(y, pal);
    click1_witness(x, y, mask, hit);
    if let (Some(Click1Hit::Console), true) = (hit, backdrop_console) {
        // Focus/activate the console: reassert its prompt + current input at the caret. Non-
        // destructive (submits nothing, mutates no shell state); makes the click visibly land.
        console.draw_input_line(pal);
    }
}

#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn render_service(_: usize) {
    use unaos_kernel::pal::GneissPal; // for pal.render()
    // FrameBuffer is Copy: take a handle and build the back-buffered surface. All drawing goes to
    // cached RAM; render() flushes only the damaged span to the framebuffer, cleaning the cache so
    // the (non-snooping) VideoCore scan-out sees it.
    let front_fb = *unaos_kernel::video::WRITER.lock();
    let mut screen = unaos_kernel::video::Screen::new(front_fb);

    // SHELLWIN-PI — does the Pi desktop own the backdrop on this build? Captured ONCE, exactly as the
    // x86 service captures `desktop_uefi::is_active()`, so every arm below reads one stable answer.
    //
    // When it is true the SCENE owns the glass and the live text shell comes off it. That sentence is
    // a bigger change on this arch than on x86, because on the Pi the shell's whole-panel
    // `clear_screen` was the ONLY writer that ever made a non-window panel pixel read `DESKTOP_BG` —
    // `wcf`'s chrome-truth probe reported exactly that as NOCLEAR ("no aarch64 panel-wide DESKTOP_BG
    // clear; desktop_uefi is x86+wc"), and it read as a desktop only because `console::Console::BG` and
    // `wm::DESKTOP_BG` are the same number by coincidence. The desktop write is deliberate and NAMED
    // on this arch now, which is the arc the NOCLEAR note asked for.
    //
    // NOTE the backdrop is NOT claimed here. `desktop` says this BUILD runs a desktop; the pass that
    // CLAIMS the glass is the mint arm in the drain loop, once `desktop_firmware::armed()` reports the witness
    // cascade done. Claiming it at task start would leave a backdrop going stale across the whole
    // cascade, and the first keystroke arriving before the arming would splash a whole-panel
    // `console.draw` across a scene nothing would repaint.
    #[cfg(feature = "desktop_firmware")]
    let desktop = desktop_owns_backdrop();
    #[cfg(not(feature = "desktop_firmware"))]
    let desktop = false;

    let mut pal = unaos_kernel::pal::TargetPal::new(&mut screen);
    let mut console = unaos_kernel::console::Console::new();

    // SHELLWIN-PI — the live shell's OWN compositor window. Flat locals for the service's life, for
    // the reason the x86 twin gives: a persistent `TargetPal` borrows its `Screen`, and a per-keystroke
    // `TargetPal::new` would also spam the once-per-surface `:: UI1:` line. `_shell_store` is bound and
    // never read again ON PURPOSE — the `wm` row holds a raw pointer into its heap buffer, and this
    // task never returns, so the binding IS the lifetime. Knob-off (`desktop_firmware` off) not one of these
    // bindings exists and the shell keeps painting the panel exactly as it always did.
    // They start EMPTY and cost nothing: the window is minted LATER, by the lazy arm in the drain
    // loop, once `video::desktop_firmware::armed()` says the boot witness cascade has released the panel. See
    // that arm — the ordering is not a nicety, it is 4 of the 108 regression witnesses.
    #[cfg(feature = "desktop_firmware")]
    let (mut _shell_store, mut shell_screen, mut shell_console, mut shell_id) = (
        alloc::vec::Vec::new(),
        unaos_kernel::video::Screen::direct(unaos_kernel::video::FrameBuffer::new()),
        unaos_kernel::console::Console::new(),
        unaos_kernel::video::wm::WIN_NONE,
    );
    #[cfg(feature = "desktop_firmware")]
    let mut shell_pal = unaos_kernel::pal::TargetPal::new(&mut shell_screen);
    #[cfg(feature = "desktop_firmware")]
    let mut shell_dirty = false;
    // One decline is final. `open_shell_window` already named the reason on the wire; retrying it
    // every pass would turn a full heap into a serial flood.
    #[cfg(feature = "desktop_firmware")]
    let mut shell_declined = false;

    // SHELLWIN-PI — UNCHANGED, and deliberately so. At boot the shell still draws the panel on every
    // build: the whole-panel `console.draw` that made the shell the Pi's wallpaper stops being the
    // desktop at the ARMING pass, not here, because until the witness cascade has released the panel
    // there is no desktop for it to be off of. This is the same pre-takeover state x86 shows.
    console.draw(&mut pal);
    // PI-UI-2: the always-on GUI status strip (hostname / lease IP / UTC wall clock). Drawn AFTER the
    // console each frame so it sits on top; refreshed at ~1 Hz by the `status_tick` task, which pings
    // GUI_CHANNEL with an Event::Timer so this loop re-renders even with no keyboard input. Reads only
    // public snapshot accessors (clock::now / net_phy::settled_ipv4) — no net/clock lock in this path.
    unaos_kernel::ui_status::draw(&mut pal);
    pal.render();
    serial_println!(":: UI2: status strip armed (host+ip+time, 1 Hz) ::");

    // PIUSB-24: whether the cursor was drawn last pass, so the auto-hide transition erases the sprite
    // exactly once when the ~1.5 s idle expires (mirrors the x86 console loop's CURSOR-HIDE). The
    // status_tick Timer pulse (~1 Hz) provides the periodic wake this check rides on.
    let mut cursor_was_visible = false;

    // SCHED-6 — dirty-flag frame pacing. The pre-SCHED-6 loop recomposed the status strip
    // (`ui_status::draw`: a heap `format!` of host/ip/clock + a full-width band fill + a glyph run)
    // AND presented (`pal.render`) on EVERY inbound event. A USB mouse re-emits an interrupt-IN
    // report every 8-10 ms whether or not it moved (and some send a null Mouse{0,0} at rest), so at
    // idle the render core recomposed the strip ~100-125×/s for no visible change — c0 pegged at
    // ~96-100% (P33). Now: pointer/button events move only the (cheap) cursor sprite and present;
    // the strip is recomposed ONLY when its content can have changed — the 1 Hz status-tick Timer
    // pulse — or when a Key redrew the console beneath it (so it stays on top). No-op pointer reports
    // (null relative motion / an unchanged absolute position) draw nothing and do not present. A pass
    // presents at most once, and only if something was actually drawn. Cursor latency is preserved:
    // a real pointer report still redraws the sprite and presents in the same pass.
    let mut last_abs: Option<(i32, i32)> = None;
    // [sched6] witness accumulators (this task is the sole owner of the render core — plain locals,
    // no atomics). Bracket each pass; report passes/s, presented composites/s, and the mean cycle
    // cost of a presented pass, rate-limited to once every ~5 s.
    let mut s6_passes: u64 = 0;
    let mut s6_composites: u64 = 0;
    let mut s6_cyc: u64 = 0;
    let mut s6_last_ms = unaos_kernel::arch::ms();

    loop {
        // Block until an event arrives (recv parks the task — an idle render core burns nothing).
        let ev = GUI_CHANNEL.recv();
        GUI_RECV.fetch_add(1, core::sync::atomic::Ordering::Relaxed); // GUI-CLICK-2 depth accounting
        let t0 = unaos_kernel::arch::now_cycles(); #[cfg(feature = "livecon")] unaos_kernel::video::fbcon::console_live_service(); // LIVECON — THE PORT. The console window's presents come off print context (`fbcon::present_deferred`) and land HERE, on the one core that drives the compositor, at this pass's cadence — which is the arc `video/desktop_firmware.rs`'s live-console ledger names. `console_live_service` is a readback bracket around the hook x86's `usbdebug` service loop has called since FBCON-PACE (`fbcon::console_service`) — the service call is unchanged and the bracket only counts presents that actually happened, which is what the one-line `[wc-x] livecon census` proof is read off. It is the PACED take, not a forced flush: `console_service` can only move a present earlier within the frame it was already going to happen in, it is free on a clean ledger (`pend_take` -> `Owed::Nothing`, nothing composited), and it is a no-op on every build where the console is not routed into a window. Top of the pass and not the tail, because a pass has many exits and a deferred band must not be able to miss one. ⚠ FOLDED onto an existing line — `main.rs` is compiled into the knob-off `kernel8.img` whose byte-identity is this track's standing proof and panic `Location` records embed line numbers (PARITY §5.3); knob-off the statement does not exist and no line moves.
        s6_passes += 1;
        // `dirty` — did this pass draw anything that must be presented? `strip_dirty` — must the
        // status strip be (re)composed on top this pass?
        let mut dirty = false;
        // PULSE-STRIP: the strip has two redraw reasons and they are NOT the same reason.
        // `strip_force` — something repainted the band underneath it (a console redraw on Key, a view
        // redraw on Button); the strip owes an unconditional redraw on top, from cached loads.
        // `strip_tick`  — the 1 Hz refresh pulse; sample the meters and redraw ONLY if the composed
        // line or a quantized per-core load changed. This is the dirty-pacing split.
        let mut strip_dirty = false;
        let mut strip_tick = false;
        // SHELLWIN-PI — **is the shell a WINDOW as of this pass?** ONE predicate, read by both input
        // arms below, and it is deliberately NOT `desktop`. `desktop` says this BUILD runs a desktop;
        // this says the hand-off has actually HAPPENED. Between task start and the arming pass the
        // shell still owns the panel — the honest pre-facade state — so until then the keystroke and
        // the click must both go on behaving exactly as they did before this arc. Sampled HERE, ahead
        // of the mint arm further down that can flip `shell_id` within this same pass, so every arm in
        // one pass agrees about which surface it is talking to.
        #[cfg(feature = "desktop_firmware")]
        let windowed = desktop && shell_id != unaos_kernel::video::wm::WIN_NONE;
        #[cfg(not(feature = "desktop_firmware"))]
        let windowed = false;
        match ev {
            unaos_kernel::pal::Event::Key(c) => {
                // SHELLWIN-PI — route the keystroke to its home. A key reaches this task only when
                // `pump_usb_into_gui` found `user_input_active() == 0`, i.e. the keyboard belongs to
                // the SHELL. On the Pi desktop the shell is a WINDOW, so it is dispatched into that
                // window's own `Console`/`TargetPal` and the backdrop scene is never typed on; the
                // window's surface is flushed and composited once per drained burst at the tail. Off
                // the desktop (`desktop_firmware` off, or the window declined) the shell is still the panel and
                // this is the pre-arc line unchanged, `dirty`/`strip_dirty` and all.
                //
                // Note which flags do NOT get set on the window path: the PANEL was not drawn on, so
                // there is nothing for `pal.render()` to flush and no strip band to repair. Setting
                // `dirty` there would present the whole backdrop on every keystroke — the exact
                // per-key full-panel cost SCHED-6's dirty pacing exists to refuse.
                if windowed {
                    #[cfg(feature = "desktop_firmware")]
                    {
                        handle_key(c, &mut shell_console, &mut shell_pal);
                        shell_dirty = true;
                    }
                } else {
                    handle_key(c, &mut console, &mut pal);
                    // The console may repaint into the strip's bottom band — redraw the strip on top.
                    dirty = true;
                    strip_dirty = true;
                }
            }
            // CURSOR-1: pointer motion moves the SHARED pointer state (`pal::cursor`, which
            // `click1_dispatch` and the compositor both read) and repaints the system cursor into
            // the FRONT framebuffer via `video::cursor` — save-under, one glyph cell, on top of the
            // console and every window. It deliberately does NOT set `dirty`: the sprite bypasses
            // the `Screen` back buffer entirely, so a pointer report costs a save + a few small
            // fills over one cell, not a `Screen::flush` of the sprite's damage box. The pre-CURSOR-1
            // code drew into the back buffer and erased with a hard-coded 0x1E1E1E, which is neither
            // the desktop colour nor on top of a window.
            unaos_kernel::pal::Event::Mouse { x, y } => {
                // Relative motion (boot-mouse proto). A null report (no delta) is the idle-mouse
                // keep-alive — draw nothing, do not present.
                if x != 0 || y != 0 {
                    unaos_kernel::pal::cursor::move_rel(
                        x,
                        y,
                        pal.width() as i32,
                        pal.height() as i32,
                    );
                    unaos_kernel::video::cursor::repaint();
                }
                #[cfg(feature = "desktop_firmware")] unaos_kernel::video::wm::drag_route_tail(ev); // DRAG-PI M3 — STEER a live grab from the SHELL path, and it has to be HERE rather than in the drain that forwarded this event. On this arch the shell's cursor is applied one `GUI_CHANNEL` hop downstream — by this very arm — so a tail call in `pump_usb_into_gui` would read the position as of the PREVIOUS report and the window would trail the arrow by one report for the whole gesture. The render task is where the cursor becomes current, so it is where the window is allowed to follow it. Reached for kernel-band and focus-exempt rows, whose chrome arm hands the keyboard to the shell.
            }
            unaos_kernel::pal::Event::MouseAbsolute { x, y } => {
                // Absolute report (0..=32767 HID space), same shared sprite. An unchanged position
                // is an idle keep-alive — skip it (no motion, no repaint).
                if last_abs != Some((x, y)) {
                    last_abs = Some((x, y));
                    unaos_kernel::pal::cursor::set_abs(x, y, pal.width() as i32, pal.height() as i32);
                    unaos_kernel::video::cursor::repaint();
                }
                #[cfg(feature = "desktop_firmware")] unaos_kernel::video::wm::drag_route_tail(ev); // DRAG-PI M3 — the absolute twin of the arm above, same reasoning and same seam.
            }
            // GUI-CLICK-1: a Button report carries no cursor motion — dispatch it against the shared
            // GUI model at the current sprite position (hit-test → deliver to the hit view). Press
            // edges only; the console's activation is a non-destructive focus/redraw of its input
            // line. Emits the rate-limited `[click1]` witness. Key/motion paths are untouched.
            unaos_kernel::pal::Event::Button(mask) => {
                click1_dispatch(mask, &console, &mut pal, !windowed);
                dirty = true;
                // A click may activate/redraw a view under the strip band — keep the strip on top.
                strip_dirty = true;
            }
            // Timer (the 1 Hz status-tick pulse) is the strip's own refresh cadence: recompose it so
            // the clock/lease advance even with no input. Other events carry nothing to draw.
            unaos_kernel::pal::Event::Timer => {
                strip_tick = true;
            }
            _ => {}
        }
        // SERIAL-FOCUS — **THE CONSUMER SIDE OF THE SOURCE SPLIT.** The shell's serial inbox is
        // drained here, on every pass, unconditionally, and the bytes are dispatched through the
        // SAME `handle_key` the USB keyboard reaches — into whichever surface `windowed` says the
        // shell is on, so a serial keystroke and a USB keystroke can never disagree about where the
        // shell lives. This is the "always reaches the shell" half of the arc: nothing here consults
        // `user_input_active()`, because a byte that got this far was never in the focus-routed
        // pipeline to begin with.
        //
        // POSITION IS THE DESIGN, and it is AFTER the `match`, not before it. The `Event::Key` arm
        // above can enter `dispatch_command` and stay there for the whole life of a foreground
        // command — the state in which `render_service` cannot `recv` and in which the pre-arc code
        // deadlocked its own input task. Draining HERE means the very pass that returns from that
        // command takes the entire backlog that accumulated during it, before parking again: a
        // command typed over the wire while an app owned the panel executes the instant the app
        // exits, with no wake token needed and no ~1 Hz strip pulse to wait for. Draining before the
        // `match` would have missed exactly that case, which is the case the blocker is about.
        //
        // The flag is cleared BEFORE the drain, never after: a byte arriving mid-drain must be able
        // to arm a fresh wake, or it could be the one that sits in the ring until the next unrelated
        // event. Clearing after would swallow that arming.
        //
        // BOUNDED ON THIS SIDE TOO, at one ring-full per pass. `offer` can keep running on the input
        // core while this drains, so an unbounded `while let` is a loop whose exit condition another
        // task controls — the shape of every freeze this campaign has had to unpick. `CAP` is the
        // most that can be owed at the top of any pass; anything offered after that is this pass's
        // successor's work, and the ring still holds it in order.
        SERIAL_WAKE_PENDING.store(false, core::sync::atomic::Ordering::Release);
        let mut serial_budget = unaos_kernel::arch::serial::shell_inbox::CAP;
        while serial_budget > 0 {
            let Some(b) = unaos_kernel::arch::serial::shell_inbox::take() else { break };
            serial_budget -= 1;
            if windowed {
                #[cfg(feature = "desktop_firmware")]
                {
                    handle_key(b, &mut shell_console, &mut shell_pal);
                    shell_dirty = true;
                }
            } else {
                handle_key(b, &mut console, &mut pal);
                dirty = true;
                strip_dirty = true;
            }
            serfocus_witness();
        }
        // CURSOR-HIDE: take the sprite off the panel once when the auto-hide delay expires
        // (reappearance is instant — the move_rel/set_abs arms above stamp the activity clock before
        // drawing). CURSOR-1: `undraw` restores exactly the pixels the sprite covered, so the hide
        // needs no present of its own — hence no `dirty`.
        let cursor_vis = unaos_kernel::pal::cursor::visible();
        if cursor_was_visible && !cursor_vis {
            unaos_kernel::video::cursor::undraw();
        }
        cursor_was_visible = cursor_vis;
        // Recompose the strip only when it was overdrawn (Key/Button) — unconditional, cached loads.
        if strip_dirty {
            unaos_kernel::ui_status::draw(&mut pal);
            dirty = true;
        } else if strip_tick {
            // PULSE-STRIP: the paced path. `tick` samples the per-core meters at most once a second
            // and returns whether it actually drew; an unchanged strip on an unchanged panel adds no
            // present at all, which is what keeps the always-running pulse off the render core's back.
            dirty |= unaos_kernel::ui_status::tick(&mut pal); #[cfg(feature = "desktop_firmware")] unaos_kernel::video::pulsewin::service(); // PULSEWIN — the pulse WINDOW is a second VIEW of the numbers `tick` has just published, so it repaints on the same cadence from the same envelope and never takes a second sample. It presents through its own `wm` row, not through `pal`, so it neither reads nor sets `dirty`. ⚠ FOLDED onto this line rather than added below it: `main.rs` is compiled into the knob-off `kernel8.img` whose byte-identity is this track's standing proof and panic `Location` records embed line numbers — PARITY.md §5.3.
        }
        // SHELLWIN-PI — MINT THE SHELL WINDOW, once, on the first pass after the desktop is armed.
        //
        // This lateness is the whole correctness of the arc on this arch, and it was MEASURED, not
        // designed: the first cut minted the window at the head of this task and
        // `UNAOS_PIDESK=1 ./arroyo kernel8-test` answered `MBENCH FAIL — 104/108`. The four misses
        // were the boot WITNESS CASCADE, not the shell — `[wc-j] vacate` and `[wc-j] move-once` read
        // vacated boxes back expecting `DESKTOP_BG` and found window (`overlap_px=30336`), `[wc-f]
        // twin` never cleared its probe strip, and `[clickroute]`'s hit-test lost its topmost row.
        // A half-panel row that exists from line 251 of the boot log sits across all of it.
        //
        // x86 never had to think about this because its latch already encodes the ordering: the
        // Kepler takeover happens after the cascade, so there "is the desktop up?" and "has the
        // cascade released the panel?" are accidentally the same question. On the Pi they are not,
        // and `video::desktop_firmware::armed()` is the second question asked in its own right.
        //
        // The wake is free: the strip pulse posts an `Event::Timer` every `PSTRIP_PERIOD_MS` from a
        // cooperative `yield_now` loop that needs no timer IRQ, so this fires within ~250 ms of the
        // cascade ending on metal and in QEMU raspi4b alike — no polling task, no new event variant.
        #[cfg(feature = "desktop_firmware")]
        if desktop
            && shell_id == unaos_kernel::video::wm::WIN_NONE
            && !shell_declined
            && unaos_kernel::video::desktop_firmware::armed()
        {
            let info = front_fb.info();
            match open_shell_window(info.width, info.height) {
                Some((store, fb, id)) => {
                    // SHELLWIN-PI — the BACKDROP hand-off, in the SAME PASS as the window. Until this
                    // moment the shell owned the panel exactly as it did before this arc (pre-arm is
                    // the Pi's honest "plumbing, not facade" state, and it is what x86 shows before
                    // its own takeover); from this moment the SCENE owns it. Claiming it HERE rather
                    // than at task start is what keeps the two coherent: a backdrop claimed at start
                    // goes stale across the whole cascade, and the first keystroke to arrive before
                    // the arming splashes a whole-panel `console.draw` over a scene nothing repaints.
                    //
                    // `clear_screen(DESKTOP_BG)` through the pal IS `Screen::paint_desktop_scene`:
                    // that method is `back.fill_screen(DESKTOP_BG)` + `mark_full()`, and
                    // `TargetPal::clear_screen` is `surface.fill_screen`, which is the same pair. The
                    // named method cannot be reached from here because `pal` holds the `&mut Screen`
                    // for this task's life — which is also why `screen.rs` needs no edit on this arc.
                    // When the lake scene replaces the flat fill the seam is one `TargetPal`
                    // passthrough, and this call site changes by one word.
                    pal.clear_screen(unaos_kernel::video::wm::DESKTOP_BG); unaos_kernel::video::retire_desktop_chrome(pal.width() as usize, pal.height() as usize); // REALDESK — the SAME statement retires the legacy backdrop tenants (`ui_status`'s pulse band and status line). It is here and not at task start for the reason the clear is: this is the only instant that is BOTH after the cascade and after `open_shell_window` actually returned a window, so on a panel whose decline leaves the shell owning the backdrop the instrument stays exactly where it is. ⚠ FOLDED — `main.rs` is compiled into the knob-off `kernel8.img` whose byte-identity is this track's standing proof and panic `Location` records embed line numbers; the argument is in PARITY.md §6.1 and at the tail of `video/mod.rs`.
                    dirty = true;
                    // Rebind the whole tuple, the shape the x86 reopen arm established: the store
                    // must outlive the row (which holds a raw pointer into it), the `Screen` is
                    // `direct` — NEVER `Screen::new`, whose infallible second surface-sized buffer is
                    // the allocation that OOM-panicked GR26's metal boot, and which on this arch
                    // would ask a 48 MiB heap for 2.2 MB it does not need — and the `TargetPal`
                    // re-borrows the new `Screen` (one `:: UI1:` line, the once-per-surface rule).
                    _shell_store = store;
                    shell_console = unaos_kernel::console::Console::new();
                    shell_console.mark_in_window();
                    shell_screen = unaos_kernel::video::Screen::direct(fb);
                    shell_pal = unaos_kernel::pal::TargetPal::new(&mut shell_screen);
                    shell_id = id;
                    // First frame: the prompt into the window's own surface, flushed and composited
                    // once through the owner-fenced present, so the operator has a live shell to
                    // click and type into from the moment the desktop appears.
                    shell_console.draw(&mut shell_pal);
                    shell_pal.render();
                    let _ = unaos_kernel::video::wm::present_outcome_owned(
                        id,
                        unaos_kernel::video::wm::KERNEL_OWNER_DESKTOP,
                    );
                    shell_dirty = false;
                    serial_println!(
                        "[shellwin-pi] backdrop=crispy-scene shell=window win={} surf={}x{} panel={}x{} == witness ::",
                        id,
                        shell_pal.width(),
                        shell_pal.height(),
                        pal.width(),
                        pal.height()
                    );
                }
                None => {
                    // The REASON is already on the wire from `open_shell_window`; this is the
                    // CONSEQUENCE, so a capture never has to infer "and therefore the keyboard has
                    // nowhere to land". The scene keeps the glass; nothing is torn down.
                    shell_declined = true;
                    serial_println!("[shellwin-pi] backdrop=crispy-scene shell=none (window declined) ::");
                }
            }
        }
        // Present at most once per pass, and only when something was actually drawn — a no-op report
        // costs a bounded match + a couple of cycle reads, not a composite.
        if dirty {
            // CURSOR-1: `Screen::flush` copies back-buffer pixels over the front framebuffer's
            // damaged rects, which would both clobber the sprite and invalidate its save-under. Take
            // it off first, present, put it back on top.
            //
            // CURSOR-13: that bracket now lives INSIDE `Screen::flush`, around the desktop blit
            // alone, and it is deliberately not restated here. `flush` is `present_background` (a raw
            // desktop blit, which still needs the bracket) FOLLOWED BY the window composite (which
            // must NOT be inside one). Wrapping the pair from out here put every flush-reached
            // composite between an `undraw` and a `repaint`, so `cursor::sprite_plan()` returned
            // `None` on 100% of those passes and compose-through could never engage — P74's
            // `[cursor12] -> nosprite`, on both arches, for structural reasons. See `Screen::flush`
            // for the full single-owner argument. The cost is unchanged (one restore/save/draw per
            // present, just scoped to the half that needs it), and every other flush site on this
            // arch — the boot present above, `rast_demo`, the `video::witness` fixtures — now gets
            // the desktop bracket without having to remember it.
            pal.render();
            s6_composites += 1;
        }
        // SHELLWIN-PI — flush the shell window's surface and composite it once per pass, AFTER the
        // backdrop present above so the window lands on top of the scene rather than under it. Same
        // drain-then-present-once discipline the backdrop follows.
        //
        // The verb is `present_outcome_owned`, never bare `wm::present`, and the reason is the
        // recycled-slot hazard the x86 review convicted: `shell_id` is a slot alias with no
        // generation, so a fence-free present would composite whatever row later takes the slot. The
        // fence answers `NoRow` when the row was reaped and `Suppressed` when it is parked, and is
        // identical to a plain present while the shell is live. On this arch that fence has a second
        // duty: kernel furniture is NOT closable (`wc_close_click` returns `furniture-refused`), and
        // PA38 is the ruling that a REFUSED close must leave no orphaned state behind it — a present
        // that answers honestly about a row it does not own is the same discipline one layer down.
        //
        // Deliberately NOT counted into `s6_composites`: that witness measures PANEL composites, and
        // folding a window present into it would report the backdrop presenting when it did not.
        #[cfg(feature = "desktop_firmware")]
        if shell_dirty {
            shell_pal.render();
            let _ = unaos_kernel::video::wm::present_outcome_owned(
                shell_id,
                unaos_kernel::video::wm::KERNEL_OWNER_DESKTOP,
            );
            shell_dirty = false;
        }
        s6_cyc += unaos_kernel::arch::now_cycles().wrapping_sub(t0);
        // Rate-limited [sched6] witness: incoming pass rate vs presented-composite rate + mean pass
        // cost over the window (proves the pacing — presents track real activity, not the event rate).
        let now = unaos_kernel::arch::ms();
        let span = now.wrapping_sub(s6_last_ms);
        if span >= 5000 {
            let passes_per_s = s6_passes.saturating_mul(1000) / span.max(1);
            let comps_per_s = s6_composites.saturating_mul(1000) / span.max(1);
            let mean_cyc = s6_cyc / s6_passes.max(1);
            serial_println!(
                // PULSE-4: the strip's cadence moved to 4 Hz; this witness's own 5 s span is
                // deliberately UNCHANGED (wire volume). The label names the strip's rate, not this
                // line's, so it tracks `PSTRIP_PERIOD_MS` rather than restating a stale literal.
                "[sched6] passes={}/s composites={}/s mean={} cyc/pass (dirty-paced strip@{}ms)",
                passes_per_s,
                comps_per_s,
                mean_cyc,
                unaos_kernel::ui_status::PSTRIP_PERIOD_MS
            );
            // SCHED-PRIO: the dispatch-share line, emitted on `[sched6]`'s cadence and immediately
            // after it, so a capture reads "composites=N/s" and "who won the dispatches that produced
            // them" as one pair. This is the only site that fires while a fleet is actually live —
            // the scheduler's own two emitters are metal-only and boot-once respectively.
            unaos_kernel::arch::sched::prio_witness();
            s6_passes = 0;
            s6_composites = 0;
            s6_cyc = 0;
            s6_last_ms = now;
        }
    }
}

/// PI-UI-2 status-strip refresh pulse (metal only): once per `ui_status::PSTRIP_PERIOD_MS` (PULSE-4:
/// 250 ms, was a hard-coded second — see that module's latency-budget note), post an `Event::Timer` to
/// GUI_CHANNEL so the render task re-draws the status strip (lease IP / wall clock advance) even when
/// no keystroke is arriving. Mirrors the `rx_backstop` shape — a tiny periodic wake, off the render
/// core's critical path. Gated on `timer::is_live()` at spawn (a `sleep_ticks` nap needs the timer
/// IRQ to wake); in QEMU raspi4b (no Group-1 IRQ) it is not spawned and the strip refreshes on input.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn status_tick(_: usize) {
    loop {
        // PULSE-4: the wake cadence is DERIVED from the strip's own sample period, not hard-coded
        // beside it. This was `sleep_ticks(250)` — a literal second — and it is the OUTER term of the
        // strip's latency budget: raising `PSTRIP_PERIOD_MS` alone would have left metal sampling at
        // 1 Hz regardless, i.e. PULSE-4 doing nothing on the only machine whose panel Peter watches.
        unaos_kernel::arch::sched::sleep_ticks(unaos_kernel::ui_status::PSTRIP_PERIOD_TICKS);
        // GUI-CLICK-2: suppress the status-strip pulse while a full-screen app owns the screen.
        // render_service is blocked inside dispatch_command and cannot drain GUI_CHANNEL, so an
        // ungated Timer would fill the 64-slot channel in ~16 s at PULSE-4's 4 Hz — a re-run of the exact
        // saturation the pointer-path gate prevents (and the strip is not visible under the app
        // anyway). The pulse resumes the instant the command returns.
        // GUI-WIRE: the watchdog escape hatch. If the active full-screen app has stopped making
        // drain progress (poll() latches its own wedge witness), return input to the shell so the
        // keyboard is never trapped inside a dead app. No-op when no app owns the screen.
        if unaos_kernel::gui_watchdog::poll() {
            SCREEN_APP_ACTIVE.store(false, core::sync::atomic::Ordering::Relaxed);
        }
        if !SCREEN_APP_ACTIVE.load(core::sync::atomic::Ordering::Relaxed) {
            gui_send(unaos_kernel::pal::Event::Timer);
        }
    }
}

/// PIUSB-26 (metal only): the xHCI event pump on its own cadence. PIUSB-23 wired `pump_usb_into_gui`
/// into the input service's UART wake, so pointer/key events only drained on an RX interrupt or the
/// ~5 Hz rx-backstop poke — batching a moving mouse to ~5 fps ("very very slow" on metal, P33). This
/// dedicated task pumps at ~4 ms (`sleep_ticks(1)` at the 250 Hz per-core tick, matching interrupt-EP
/// intervals of 8-10 ms), so ~60+ pointer reports/s reach the render task while the UART/keyboard
/// interrupt path stays untouched. No busy-spin — it naps between passes. Gated on `timer::is_live()`
/// at spawn like `rx_backstop`/`status_tick`; in QEMU raspi4b the input task's poll-nap fallback still
/// pumps each cooperative pass. A rate-limited `[piusb26]` witness proves the idle-controller cost of a
/// pass is micro (a `poll_events` on an empty event ring is cheap MMIO). Never returns.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
fn usb_pump(_: usize) {
    use core::sync::atomic::Ordering;
    loop {
        unaos_kernel::arch::sched::sleep_ticks(1); // ~4 ms at the 250 Hz per-core tick
        let t0 = unaos_kernel::arch::now_cycles();
        pump_usb_into_gui();
        let dt = unaos_kernel::arch::now_cycles().wrapping_sub(t0);
        // Rate-limit the cost line to once every ~5 s (this loop runs ~250×/s).
        let now = unaos_kernel::arch::ms();
        let last = PIUSB26_LAST_LOG_MS.load(Ordering::Relaxed);
        if now.wrapping_sub(last) >= 5000 || last == 0 {
            PIUSB26_LAST_LOG_MS.store(now.max(1), Ordering::Relaxed);
            serial_println!("[piusb26] pump pass {} cyc (~4 ms cadence, idle controller)", dt);
        }
    }
}

// ── SCHED-X86 ───────────────────────────────────────────────────────────────────────────────────
// The x86 GUI handoff, mirroring the Pi's M5/M5b/PIUSB-26 split (`usb_pump` / `input_service` /
// `render_service` above). Until this arc the x86 BSP fell into an inline GUI loop at the end of
// `kernel_main` and NEVER entered the scheduler, which had two consequences the metal capture named:
// every `bg`/`run` placed its ring-3 task on the calling core (core 0, `bg_place_cpu` =
// `meter_current_cpu`), where nothing ever popped the run queue — 2 BGRUN spawns, zero
// `wc-x86: SYS_WIN_CREATE`, zero `:: SYSCALL:` witnesses, both kills burning the full
// `KILL_CONFIRM_MS` — and the pulse meter read `c0:0/0` while every other core read `0/263`.
//
// The three tasks below dismantle that loop into scheduled kernel services and the BSP joins the
// scheduler via `sched::run_bsp(0)`. Two placement rules are LOAD-BEARING and are asserted at the
// spawn site rather than left to comments:
//
//  1. `x86_usb_pump` and `x86_render_service` MUST be on DIFFERENT cores. `XHCI_CONTROLLER` is a raw
//     `spin::Mutex`, not the scheduler's sleeping `Mutex`, and both tasks take it (the pump directly;
//     the render side transitively, through `fat` block reads, `pal::pump_and_poll` inside a
//     full-screen app, and the `usbinfo` verb). Kernel tasks ARE preempted (`timer_preempt` acts on
//     any `current`, not just ring 3), so co-locating two preemptible takers of a raw spinlock on one
//     core is a hard deadlock: preempt the holder and the spinner — which cannot yield — owns the
//     core forever. Cross-core it is bounded spin, which progresses. No future task that touches xHCI
//     may be added to the render core.
//
//     WITCORE: "asserted at the spawn site" held for these three tasks and NOWHERE ELSE, which is how
//     the rule was already being broken by tasks spawned later: `irqstorage`'s `storage-svc` (a
//     preemptible `XHCI_CONTROLLER` taker) placed itself on `online_aps().first()` — i.e. the render
//     core — and the whole cooperative ring-3 fixture ladder did the same. The rule now lives in
//     `arch::smp` (`worker_cpu` / `xhci_worker_cpu`), the split is published to it by
//     `smp::publish_sched_split` below, and the resulting map is printed once as
//     `:: SCHED-X86 PLACE: ... ::`. Ask that module for a core; do not re-derive one from
//     `online_aps()`.
//  2. `x86_input_service` PAINTS NOTHING. On x86 `pal::cursor::SPRITE_OWNS_PAINT` is true, so the
//     cursor verbs drive the compositor sprite straight into the FRONT buffer; running them on the
//     input core would put two cores on the panel. The routers (`wc_click_route`, `user_input_route`)
//     move with the pixels for the same reason — `wc_click_route` mutates window-manager focus and
//     repaints through `wm::focus_changed`, so it belongs on the render side. The input task is a
//     pure forwarder, exactly like the Pi's.

/// KEYREPEAT-X86 (Boot AL) — the x86 twin of the aarch64 pump's typematic call: synthesise a held
/// key's repeat into `EVENT_QUEUE` once per device-service pass.
///
/// WHY IT IS A FUNCTION AND NOT AN INLINE CALL. x86 has THREE mutually-exclusive per-pass service
/// loops that poll `ehci::service_ehci_hid` — the `usbdebug` terminal loop, the inline BSP console
/// loop (taken when fewer than two APs came online, so the render/service split cannot be made), and
/// `x86_usb_pump` (the SCHED-X86 device-service task, the normal desktop path). A repeat that only
/// fired on one of them would be a boot-configuration-dependent keyboard, which is precisely the
/// class of divergence GR21 spent two arcs removing from this driver. One body, three call sites.
///
/// PLACEMENT WITHIN A PASS mirrors aarch64 exactly: AFTER the HID service call that pushes this
/// pass's genuine edges, so the tracker has already seen this pass's reports, and BEFORE any drain,
/// so the injected `Event::Key` rides the identical routing a real press takes — `x86_input_service`
/// forwards it over `GUI_CHANNEL_X86` to the render task, `wc_click_route`/`user_input_route` apply
/// the same asid focus rules, and a focused ring-3 app receives it in its own per-process ring. No
/// per-path code, no second routing policy, and `typematic_tick`'s own backpressure guard refuses to
/// inject at all while `EVENT_QUEUE` is past half full, so a stuck repeat can never starve real HID.
///
/// A no-op without `ehcihid`: the tracker is compiled on x86 only alongside the decoder that feeds
/// it (`pal.rs` §KEYREPEAT-X86), so a build without the EHCI HID path has no keyboard to repeat for.
#[cfg(target_arch = "x86_64")]
fn x86_typematic_pump() {
    #[cfg(feature = "ehcihid")]
    if let Some(k) = unaos_kernel::pal::typematic_tick() {
        // WITNESS, once per boot. The `[keystat] typematic hold end` rollup already reports per-hold
        // and per-boot repeat counts from shared code, but it only prints when a hold ENDS — so a
        // capture taken mid-hold, or a boot where the operator never lifted the key, would show
        // nothing at all and be indistinguishable from the tracker never having been reached. This
        // line answers "did x86 synthesise a repeat, ever" at the first repeat, which is the single
        // fact this arc must be falsifiable on.
        static FIRST: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
        if !FIRST.swap(true, core::sync::atomic::Ordering::Relaxed) {
            serial_println!(
                ":: KEYREPEAT-X86: first synthesised repeat — key={:#04x} '{}' (host typematic armed on the EHCI keyboard) == witness ::",
                k,
                if (32..127).contains(&k) { k as char } else { '.' }
            );
        }
        unaos_kernel::pal::push_event(unaos_kernel::pal::Event::Key(k));
    }
}

/// SCHED-X86: the DEVICE-SERVICE task — every per-pass call the dismantled BSP GUI loop made that
/// touches hardware and no pixel. Owns the xHCI service family, the FAT/flight-recorder/boot-ledger
/// pumps, the one-shot witness probes and the NIC drain. Pinned to the service core; never returns.
///
/// Cadence: `sleep_ticks(1)` ≈ 1 ms at the calibrated 1 kHz local-APIC tick. That is deliberately the
/// SAME floor the old loop had — it ended each pass in `hlt()`, which the periodic timer broke once
/// per tick — so this is a faithful translation of the service rate and not a boot-pace regression.
#[cfg(target_arch = "x86_64")]
fn x86_usb_pump(cpu: usize) {
    serial_println!(":: SCHED-X86: usb-pump task dispatched on core {} ::", cpu);
    loop {
        // Nap first: `spawn` puts us on the run queue immediately, and the framebuffer handoff on the
        // BSP is still finishing. One tick costs nothing and keeps the first pass off that seam.
        unaos_kernel::arch::sched::sleep_ticks(1);
        // Poll xHCI, then run any deferred storage work (synchronous BOT transactions run here, in a
        // safe non-event context).
        //
        // WEDGE-8: this is a LOAN, not a lock. The controller is taken out of the shared slot under
        // a masked O(1) hold and handed back when `loan` drops at the end of this block, so the
        // multi-second BOT chain that `service_storage` can start runs with NO driver lock held —
        // which is what stops a preempted holder from deadlocking a masked FS waiter (the F3
        // family). A failed `claim()` means another context holds the loan, or the controller is not
        // installed yet; either way this pass skips, exactly as the old `if let Some` did on an
        // empty slot.
        if let Ok(mut loan) = unaos_kernel::drivers::xhci::claim() {
            let xhci = &mut *loan;
            xhci.poll_events();
            // BOOTPACE M2 — CONSOLE-FIRST: `service_ftdi` ahead of `service_storage`, so the console
            // is armed before the deferred SCSI bring-up puts its multi-second chain on the wire.
            xhci.service_ftdi();
            xhci.service_storage();
            xhci.service_hubs();
            xhci.service_hid_setproto();
            xhci.service_slot_disposal();
            xhci.service_enum();
        }
        // EHCI-3 (ehcihid knob): poll the EHCI HID interrupt endpoints (internal rMBP
        // keyboard/trackpad). Same polled-service spot as the xHCI hooks above.
        #[cfg(feature = "ehcihid")]
        unaos_kernel::drivers::ehci::service_ehci_hid();
        // WEDGEINJ (wedgeinj knob, default OFF) — keep a LIVE core asking for the gate after the
        // injected park, because the steal is a branch inside `composite` and not a timer. On metal
        // this lane supplied that condition for free (`pace_service` / `console_service` above);
        // on an idle QEMU desktop nothing asks and the steal never fires. Compiles to nothing
        // without the knob, and does nothing even with it until the fault has been injected.
        unaos_kernel::video::wm::wedgeinj_drive_maybe();
        // WCSER-REHOME: re-home the singleton roles of any core the compositor's steal has just
        // declared dead. HERE because this is the repeating service pass on the core the steal has
        // never killed, and because `spawn` must not be called from inside the composite pass where
        // the death is detected. Two relaxed loads per pass when nothing is owed.
        render_rehome_service();
        // KEYREPEAT-X86: synthesise a held key's repeat into EVENT_QUEUE, which `x86_input_service`
        // drains and forwards over GUI_CHANNEL_X86 exactly as it does a real press.
        x86_typematic_pump();
        // BATMON-1 — the SMC accumulator, restored to a path a normal GUI boot actually reaches.
        //
        // This call existed at main.rs:972, but that site sits inside `#[cfg(feature = "usbdebug")]`
        // whose loop is TERMINAL (`hlt()`, no break), so on a stock GUI boot it never ran. The only
        // other site is `pci::init` — one shot at boot, never repeated. The in-kernel vug demo had
        // the third, and ee6bfd97 deleted it. Net effect on metal: an idle desktop produced ZERO
        // `:: PWR: ::` lines for its entire uptime, while a boot with a render loop running produced
        // one every ~10 s. The rollup needs `samples > 0` and nothing was accruing them.
        //
        // `refresh_if_due` throttles itself to ~1 s of real SMC port I/O, so calling it from the
        // device-service pass costs a timestamp compare on the other passes. When the igpu lane's
        // `ui_tick_service()` lands this moves there; the call belongs on a per-pass service body
        // either way, and this task is that body.
        #[cfg(all(target_arch = "x86_64", feature = "smc"))]
        unaos_kernel::drivers::smc::battery::refresh_if_due();
        // STOR-1 (irqstorage knob): bring up the interrupt-driven storage service task once a block
        // device is present, then run the `bx-blockreq` self-test once. Both one-shot + gated.
        #[cfg(feature = "irqstorage")]
        {
            unaos_kernel::drivers::xhci::irqstorage::start_service_once();
            unaos_kernel::drivers::xhci::irqstorage::selftest_once();
        }
        // Once storage is up, mount + log the FAT volume geometry (one-shot). Runs with the xHCI lock
        // released; `read_block` re-locks it briefly.
        unaos_kernel::fs::fat::probe_once();
        // BT-BOND M1 (holocron knob): the classed-record store's whole main-loop presence — one call,
        // here, beside `probe_once` and for the same reason that one is here rather than in a driver: the
        // deferred write MUST run with no driver lock held. The Bluetooth chain that produces the record
        // this store exists for runs inside `service_ehci_hid()` holding `EHCI_HID`, and the writable FAT
        // volume rides USB mass storage serviced from that same pass — so a filesystem write issued from
        // in there would contend the xHCI storage loan from inside the EHCI service pass AND hold the
        // internal keyboard and trackpad hostage for its duration. The producer marks RAM dirty under the
        // lock; THIS call site does the I/O. `flush_if_dirty` re-checks that invariant rather than trusting
        // the placement (it DEFERS while `EHCI_HID` is busy — it does not refuse; see HCR1), so a call site that gets it wrong
        // goes loud instead of wedging. One-shot inside: the pure fixtures fire on the first pass, the load
        // latches once storage is up, and the flush costs a relaxed atomic load per iteration thereafter.
        // Like `fatverb_storage_witness`, it sits at ALL THREE storage-ready passes this file carries,
        // because which pass a given build reaches depends on its knobs.
        #[cfg(feature = "holocron")]
        unaos_kernel::fs::holocron::service();
        // PRTSCR: perform a pending Print Screen capture (`video::prtscr`). Here, beside
        // `probe_once` and `holocron::service`, and for the SAME reason those are here rather
        // than in a driver: the HID decoders detect the key edge while holding their controller
        // lock, and the writable FAT volume rides USB mass storage serviced from that same pass,
        // so an encode-and-write issued from in there would contend the storage loan from inside
        // the input pass and hold the internal keyboard and trackpad hostage for seconds. The
        // decoder sets a flag; THIS call site does the work. Ungated and arch-neutral, at all
        // THREE storage-ready passes this file carries, because which pass a given build reaches
        // depends on its knobs. Idle cost: one relaxed atomic load per iteration.
        unaos_kernel::video::prtscr::service();
        // PRTSCR-ST (prtscrst knob, default OFF): the capture's BOOT-TIME-WRITE witness — one
        // real capture through the same `capture()` the verb and the key call, then a read-back
        // of what landed on the medium. Here for the same reason `service` is here: it writes the
        // FAT volume and must do so with no driver lock held. It latches only on a pass that HAD
        // a volume, so an early pass before storage enumerates does not burn the one attempt.
        #[cfg(feature = "prtscrst")]
        unaos_kernel::video::prtscr::selftest_once();
        // SELFHOST-2 (x86, selfhost knob): the source-verify + tar walk, one-shot — see the note at
        // the first loop site. This is the pass the GUI/desktop boot reaches, i.e. the metal boot.
        #[cfg(all(target_arch = "x86_64", feature = "selfhost"))]
        unaos_kernel::selfhost::verify_source_once();
        // DESKTOP-APP (wc knob): the deferred half of kernel-apps eviction move #1. `desktop_uefi::activate`
        // used to open a kernel-drawn demo window at the Kepler takeover seam; it now ARMS a launch
        // there and this pass performs it, putting `STAT.ELF` on the desktop as a real ring-3 process
        // with a real ASID instead of ~110 lines of ring-0 furniture.
        //
        // HERE and nowhere else, for three reasons the function's own doc spells out: the launch
        // reads the FAT volume (an `XHCI_CONTROLLER` taker, which the placement rule above forbids on
        // the render core), it spawns onto `bg_place_cpu()` = the CALLER's core, so the caller must
        // be a core that actually dispatches (the BSP's inline GUI loop is not — that is the whole
        // SCHED-X86 finding), and it must WAIT for xHCI to enumerate storage, which only a repeating
        // service pass can do. One-shot inside; two atomic loads on every pass but one.
        #[cfg(feature = "wc")]
        unaos_kernel::video::desktop_uefi::desktop_app_service();
        // SDHC-4b (x86, sdhcblk knob): mount the INTERNAL SD card READ-ONLY once registered (one-shot).
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        unaos_kernel::fs::fat::sdhc_probe_once();
        // FATVERB: the shell's storage witness (one-shot) — see the note at the first loop site.
        // This file carries THREE storage-ready passes and which one a given x86 build reaches
        // depends on its knobs, so the call sits at all three and the latch inside makes it speak
        // exactly once. Mutates nothing.
        #[cfg(all(target_arch = "x86_64", feature = "witness"))]
        unaos_kernel::shell::fatverb_storage_witness();
        // WIFI-1 (wifi knob): the Broadcom/bcma firmware-load path — see the note at the second loop
        // site. Third of the three storage-ready passes; the forward-only state machine inside makes
        // it speak exactly once whichever pass a given build reaches. Read-only in arc 1.
        #[cfg(all(target_arch = "x86_64", feature = "wifi"))]
        unaos_kernel::wifi::service();
        // GUI-WITNESS M3 (witness knob): re-dump the boot-milestone ring to serial on growth.
        //
        // USBDBG-INVERT — and on `usbdebug` too, which is the ONE service the terminal loop provided
        // that this pass did not. That loop called `service_serial_dump()` UNGATED (a bring-up card's
        // recorder ring is half its value), so gating it on `witness` alone here would have made the
        // inverted card quietly lose the M3 proof path the moment it stopped looping. Same call, same
        // placement, one knob wider.
        #[cfg(any(feature = "witness", feature = "usbdebug"))]
        unaos_kernel::bootlog::service_serial_dump();
        // BPACE: re-emit the boot-phase timing ledger whenever it grows. Deliberately NOT under the
        // witness gate — the media `./arroyo esp-x86` writes carries neither `witness` nor
        // `usbdebug`, and this is the only build that reaches the bench.
        unaos_kernel::bootpace::service_dump();
        // FBCON-PACE: retire held console damage on THIS lane too. The usbdebug loop got this call
        // first, but the bench media carries `wc` without `usbdebug` — the console routes and THIS
        // pump is its only always-running service loop, so without the paced hook here a burst's
        // trailing band waits for the next print. Paced, not forced; free on a clean ledger.
        unaos_kernel::video::fbcon::console_service();
        // FLIGHT-RECORDER: flush the captured serial boot log to UNAOS.LOG on the FAT volume.
        unaos_kernel::flight_recorder::service();
        // U2/U4x/U5x/U6x/U6bx (witness knob): the ring-3 fixture ladder, each one-shot and gated on
        // storage. These used to run on the BSP; they now run inside a kernel task, which is strictly
        // better for them — `spawn_user`'s target-core choice and the bounded `ticks()` waits are
        // unchanged, and core 0's ms-clock keeps advancing underneath.
        #[cfg(feature = "witness")]
        {
            unaos_kernel::arch::syscall::u2_probe_once();
            unaos_kernel::arch::syscall::u4x_probe_once();
            unaos_kernel::arch::syscall::u5x_probe_once();
            unaos_kernel::arch::syscall::u6x_probe_once();
            unaos_kernel::arch::syscall::u6bx_probe_once();
        }
        // INSTALL-CORE (installdemo knob): run the installer engine end-to-end once a blank scratch
        // disk is present. INSTGUI supersedes it — there the attended Enter is the only trigger.
        #[cfg(all(feature = "installdemo", not(feature = "instgui")))]
        unaos_kernel::install::install_probe_once();
        // One-shot USB topology dump to serial (enumeration diagnosis; `usbinfo` shows it live).
        unaos_kernel::drivers::xhci::log_summary_once();
        // FBCON-PACE: the console's present census, once, beside the xHCI summary — same placement
        // and reasoning as the usbdebug loop's copy, because THIS is the loop the bench media runs.
        unaos_kernel::video::fbcon::console_pace_census_once();
        // Drain any frames the NIC has received into the network stack (no-op with no NIC).
        unaos_kernel::drivers::e1000::service_net();
    }
}

/// SCHED-X86: the INPUT service — drain `pal::EVENT_QUEUE` (filled by the HID decode inside
/// `x86_usb_pump`'s `poll_events`) and forward every event over `GUI_CHANNEL_X86` to the render task.
/// Paints nothing and routes nothing; never returns.
///
/// Two behaviours it carries that are not "forward an event":
///
///  * **The `SCREEN_APP_ACTIVE` gate**, taken straight from the Pi's `pump_usb_into_gui`. While a
///    full-screen command owns the panel the render task is blocked inside `dispatch_command` and
///    cannot drain the channel, so forwarding would (a) starve the app, which reads `EVENT_QUEUE`
///    itself through `pal::pump_and_poll`, and (b) fill the 64 slots and block this task in `send`.
///    While the flag is set we leave the queue alone — that IS the delivery path for those apps.
///  * **The `X86_GUI_PULSE_MS` heartbeat.** The render loop blocks on `recv`, so a periodic
///    `Event::Timer` is what lets it run the CURSOR-HIDE auto-hide erase and the `instgui` rescan
///    with no input arriving. `Event::Timer` is inert everywhere else on the path: `wc_click_route`
///    ignores non-`Button` events, and `pack_input` returns `None` for it so `user_input_route` hands
///    it straight back rather than pushing it into a focused app's ring.
///
/// # INPUT-UNGATE — THIS TASK NEVER PARKS ON THE RENDER CHANNEL
///
/// It used to. `gui_send_x86` called `Channel::send`, which blocks while the 64 slots are full, and
/// the channel's only consumer is the render task. Boot 16 (2026-08-22) is what that costs when the
/// consumer dies rather than lags: core 1 parked inside a non-returning MMIO store at 83.8 s, taking
/// the render task with it; by 101 s this task had filled the channel and parked in `send`; and
/// `[deadman] hq=25` then sat unmoving for the remaining **497 seconds** of the boot. Because this
/// task is also the sole drain of `pal::EVENT_QUEUE`, its park killed the entire input path. Input
/// was gated behind the compositor BY CONSTRUCTION, and no amount of compositor repair fixes that.
///
/// So the send is an OFFER ([`gui_try_send_x86`]) and the loop always comes back round. What happens
/// to a refused event is decided per CLASS, because the classes are not equally recoverable and
/// treating them alike is how a keystroke goes missing:
///
///  * **Relative `Event::Mouse` — COALESCED, never dropped.** Deltas are additive (`cursor::move_rel`
///    adds; `wc_drag_motion` reads the resulting ABSOLUTE position), so summing a refused run and
///    delivering the sum later steers the arrow and any live drag to the identical place. This is
///    `GUI_FOLD_X86`'s fold moved to the PRODUCER side, and it inherits that ledger's conservation
///    reasoning verbatim: a coalesced report is not a lost report, so it is counted in its own term
///    (`deadman`'s `in=` first field) and never netted out of `GUI_SENT_X86` — which counts what the
///    channel ACCEPTED and must keep meaning that for `sent - recv` to read as live occupancy.
///  * **`MouseAbsolute` is NOT folded**, for the reason the consumer-side fold gives: absolute
///    positions are not additive, and the QEMU tablet fixtures assert an exact resting coordinate.
///    It takes the must-survive path below.
///  * **`Key` / `KeyUp` / `Button` / `Wheel` / `MouseAbsolute` — MUST SURVIVE.** A refused one is
///    held in a single slot and, crucially, **we stop taking from `pal::EVENT_QUEUE`**. The ring IS
///    the backpressure: it is bounded, its overflow is already counted (`EVQ_DROP_KEY`), and leaving
///    events in it costs nothing but latency — whereas dropping them here would convert a visible
///    hang into invisible loss, which is strictly worse because it looks healthy. We return to the
///    top of the loop rather than parking, so the ring keeps being polled and, above all, **the USB
///    pump sharing this core keeps its CPU**.
///  * **`Event::Timer` — dropped freely.** It is loss-tolerant filler by contract (see the heartbeat
///    note above: every consumer on the path ignores it), so a full channel simply sheds it. Counted
///    anyway — "allowed to lose" and "lost silently" are different claims.
///
/// ORDER IS PRESERVED. Anything owed is retired before anything new is taken from the ring, folded
/// motion ahead of the held event (the fold was taken from in FRONT of it), and a heartbeat is never
/// injected while something is owed. So a `Button` can never overtake motion that preceded it, which
/// is the ordering GHOST-DRAG's cure is built on.
///
/// This is necessary and NOT sufficient on its own. A non-parking producer facing a consumer that
/// will never return just coalesces and holds forever; what actually restores input is the channel
/// getting a live consumer again, which is WCSER-REHOME's job (see `render_rehome_service_once`).
#[cfg(target_arch = "x86_64")]
fn x86_input_service(cpu: usize) {
    use core::sync::atomic::Ordering;
    use unaos_kernel::pal::Event;
    serial_println!(":: SCHED-X86: input task dispatched on core {} ::", cpu);
    let mut pulse_ms = unaos_kernel::arch::ms();
    // INPUT-UNGATE producer state. `owed_motion` is summed relative travel the channel refused;
    // `owed_event` is the ONE must-survive event it refused. Both are owed to the channel in that
    // order and nothing new is taken from the ring until both are clear.
    let mut owed_motion: Option<(i32, i32)> = None;
    let mut owed_event: Option<Event> = None;
    loop {
        // WCSER-H — the overdue probe runs FIRST, before the event pump: boot 8B proved the pump
        // can block into a wedged GUI (zero input lines within ~100ms of the hold, and the probe's
        // 5s repeats died with it — boot 8C, which survived one repeat, is the control). A probe
        // behind the pump dies with the wedge it exists to report.
        #[cfg(feature = "witness")]
        unaos_kernel::video::wm::wcser_overdue_probe();
        if SCREEN_APP_ACTIVE.load(Ordering::Relaxed) {
            // A full-screen app owns the panel and the queue. Re-base the pulse clock so the first
            // pass after it exits does not fire a stale backlog of one Timer.
            pulse_ms = unaos_kernel::arch::ms();
        } else {
            // (1) RETIRE WHAT WE OWE, oldest first. Motion ahead of the held event: the fold was
            //     taken from in front of it, so sending the event first would reorder the stream.
            if let Some((dx, dy)) = owed_motion {
                if gui_try_send_x86(Event::Mouse { x: dx, y: dy }).is_ok() {
                    owed_motion = None;
                }
            }
            if owed_motion.is_none() {
                if let Some(ev) = owed_event.take() {
                    if let Err(back) = gui_try_send_x86(ev) {
                        owed_event = Some(back);
                    }
                }
            }

            // (2) DRAIN THE RING. This runs whether or not the channel is accepting — draining is
            //     this task's other job and the one the pump's throughput depends on. It stops only
            //     when a must-survive event has nowhere to go, and then the ring holds the rest.
            while owed_event.is_none() {
                let Some(ev) = unaos_kernel::pal::next_event() else { break };
                match ev {
                    // Relative motion: fold into anything already owed, else offer it.
                    Event::Mouse { x, y } => match owed_motion.as_mut() {
                        Some(acc) => {
                            // `saturating_add` for the reason `pal`'s fold uses it: the sum is
                            // unbounded in principle and a wrapped delta would throw the arrow
                            // across the panel.
                            acc.0 = acc.0.saturating_add(x);
                            acc.1 = acc.1.saturating_add(y);
                            unaos_kernel::deadman::note_gui_coalesced();
                        }
                        None => {
                            if gui_try_send_x86(ev).is_err() {
                                owed_motion = Some((x, y));
                            }
                        }
                    },
                    // Loss-tolerant filler. The ring does not carry `Timer` today — the heartbeat is
                    // minted below, in this task — so this arm is unreachable in the current tree.
                    // It is written anyway because the POLICY is what is being stated: if a producer
                    // ever does enqueue one, it must be shed rather than allowed to occupy the slot
                    // a keystroke needs. An unreachable arm that states the rule is cheaper than a
                    // future reader inferring the rule from the absence of one.
                    Event::Timer => unaos_kernel::deadman::note_gui_timer_drop(),
                    // Everything else must survive verbatim and in order. With motion still owed it
                    // cannot go out yet even if the channel would take it — that would put it ahead
                    // of travel that preceded it.
                    other => {
                        if owed_motion.is_some() {
                            owed_event = Some(other);
                        } else if let Err(back) = gui_try_send_x86(other) {
                            owed_event = Some(back);
                        }
                    }
                }
            }

            // (3) THE HEARTBEAT. Shed it while anything is owed — it is inert filler and must never
            //     jump the queue ahead of real input — and shed it again if the channel refuses.
            let now = unaos_kernel::arch::ms();
            if now.wrapping_sub(pulse_ms) >= X86_GUI_PULSE_MS {
                pulse_ms = now;
                if owed_motion.is_some()
                    || owed_event.is_some()
                    || gui_try_send_x86(Event::Timer).is_err()
                {
                    unaos_kernel::deadman::note_gui_timer_drop();
                }
            }
        }
        unaos_kernel::arch::sched::sleep_ticks(1); // ~1 ms at the calibrated 1 kHz tick
    }
}

/// SHELLNOTDESK — does the crispy desktop compositor own the backdrop on this build/boot?
///
/// True only on x86-`wc` once [`unaos_kernel::video::desktop_uefi::activate`] has taken the panel. When it is
/// true the render service paints the CRISPY SCENE as the desktop layer and keeps the live text shell
/// off the glass — the shell is not the desktop, it is plumbing behind the facade. On a `wc`-off x86
/// build (and, by the caller's own `cfg`, aarch64 never reaches here) it is a compile-time `false`,
/// so the shell keeps painting the desktop exactly as before — the whole change folds away.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
fn desktop_owns_backdrop() -> bool {
    unaos_kernel::video::desktop_uefi::is_active()
}
#[cfg(all(target_arch = "x86_64", not(feature = "wc")))]
fn desktop_owns_backdrop() -> bool {
    false
}
/// SHELLWIN-PI — the aarch64 twin, and the one place the two arches genuinely differ in KIND.
///
/// x86 has a runtime latch because the crispy desktop arrives with a TAKEOVER: `desktop_uefi::activate` runs
/// off the Kepler display path during PCI enumeration, so "is the desktop up?" is a question with a
/// boot-time answer. The Pi has no takeover. `desktop_firmware` is a compile-time family (Cargo.toml: it
/// "declares no module of its own"), and the furniture composites from `wm::composite_once` the
/// moment the panel is up — which, by the time this task is dispatched, it is. So the honest answer
/// on this arch is the feature itself, and inventing an `AtomicBool` to launder a constant into a
/// latch would be ceremony, not information.
///
/// What x86 gets from `desktop_uefi::is_active` returning `false` — a boot that comes up with the shell still
/// on the backdrop — the Pi gets from [`open_shell_window`]'s FALLIBLE path instead: a declined
/// surface leaves `shell_id == WIN_NONE`, the scene still owns the glass, and the decline is on the
/// wire. That is the GR26 lesson applied to the arch with a 48 MiB heap (x86 has 256 MiB): the
/// window is allowed to not exist, and it is never allowed to panic the machine into existing.
#[cfg(all(target_arch = "aarch64", feature = "desktop_firmware"))]
fn desktop_owns_backdrop() -> bool {
    true
}

/// SHELLWIN — allocate the live SHELL's compositor-window surface and register its `wm` row.
///
/// SHELLNOTDESK took the interactive shell off the desktop backdrop so the crispy scene owns the glass,
/// but that left the operator with no shell to type `bg /fat/VUG.ELF` into — the keystroke path had no
/// backdrop console to reach. This gives the shell a WINDOW of its own: a kernel-owned managed row, the
/// same machinery [`unaos_kernel::video::fbcon::panel_console_window_open`] mints for the frozen
/// boot-log console, over a cached-RAM surface the render service drives a `Screen`/`Console` on.
///
/// The row's owner is [`unaos_kernel::video::wm::KERNEL_OWNER_DESKTOP`] — kernel furniture: hittable so
/// a click reaches it, out of `focus_ring`/`close_owner` so it is not a teardown victim, and routed by
/// the click router's kernel-owner arm (`wc_click_route_at`) to RAISE the row and hand the keyboard
/// back to the shell (`user_input_set_active(0)`). Focus at the shell is exactly the state in which a
/// keystroke reaches the render service's `Event::Key` arm, which is where it is routed into this
/// window's console.
///
/// Returns the surface store — the caller MUST keep it alive for the row's life, since the row holds a
/// raw pointer into it — the `FrameBuffer` over that store, and the window id. `None` when the panel is
/// unusably small or the allocation fails; the desktop then comes up with no shell window and the
/// keystroke path stays inert (the caller logs it).
///
/// SHELLWIN-PI — the gate is now `x86_64 + wc` OR `aarch64 + desktop_firmware`, and NOT ONE LINE OF THE BODY
/// CHANGED to make that true. Every verb it calls (`wm::spawn_geometry`, `wm::create_at`,
/// `ui_status::top_chrome_h`) is ungated and already compiled on aarch64, and the surface it mints is
/// its OWN heap store, so the `Bgr`/4-byte choice is a statement about the word `wm::draw_window`
/// reads (`0x00RRGGBB` little-endian) and not about the Pi panel's own pixel format. The one thing
/// that IS different is the budget: at the bench panel (1920x1200) this asks for 960x580x4 = 2.2 MB
/// of a 48 MiB heap that already carries a ~9.2 MiB panel back buffer, so `try_reserve_exact` is
/// load-bearing here in a way it never was on x86's 256 MiB.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
fn open_shell_window(
    pw: usize,
    ph: usize,
) -> Option<(
    alloc::vec::Vec<u8>,
    unaos_kernel::video::FrameBuffer,
    unaos_kernel::video::wm::WinId,
)> {
    use unaos_kernel::video::wm;
    // A terminal-sized window: roughly half the panel, floored so a small gate surface still yields a
    // usable box and clamped so the outer box (surface + chrome) fits the work area under the menu bar.
    let wtop = unaos_kernel::ui_status::top_chrome_h(pw, ph);
    let workh = ph.saturating_sub(wtop);
    let cw = (pw / 2).clamp(320, pw.max(320));
    let ch = (workh / 2).clamp(200, workh.max(200));
    let stride = cw * 4;
    let len = ch.checked_mul(stride)?;
    if cw == 0 || ch == 0 || len == 0 {
        return None;
    }
    let mut store: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    if store.try_reserve_exact(len).is_err() {
        serial_println!("[shellwin] DECLINE reason=alloc len={}", len);
        return None;
    }
    store.resize(len, 0);
    let mut surf_fb = unaos_kernel::video::FrameBuffer::new();
    // `Bgr` + 4 bytes, matching the console window: `put_pixel` stores b,g,r at bytes 0,1,2 and leaves
    // byte 3 zero — the little-endian `0x00RRGGBB` word `wm::draw_window` reads. Same bytes, no convert.
    surf_fb.init(
        store.as_mut_ptr() as usize,
        len,
        unaos_boot_info::FrameBufferInfo {
            width: cw,
            height: ch,
            stride: cw,
            bytes_per_pixel: 4,
            pixel_format: unaos_boot_info::PixelFormat::Bgr,
        },
    );
    surf_fb.fill_screen(wm::DESKTOP_BG);

    // SPAWN-PLACE — size the outer box before any row exists, then pin the row low in the work area so
    // it does not sit exactly over the centred boot-log console. `create_at` composites the row before
    // it returns (no first-frame jump), reading the `DESKTOP_BG` fill above until the caller renders the
    // prompt into it.
    let (_scale, ow, oh) = wm::spawn_geometry(cw, ch)?;
    let ox = pw.saturating_sub(ow) / 2;
    let oy = wtop + workh.saturating_sub(oh) * 3 / 4;
    let id = wm::create_at(
        wm::KERNEL_OWNER_DESKTOP,
        surf_fb.base(),
        len,
        cw as u32,
        ch as u32,
        stride as u32,
        b"shell",
        ox + wm::BORDER,
        oy + wm::TITLE_H + wm::BORDER,
    );
    if id == wm::WIN_NONE {
        serial_println!("[shellwin] DECLINE reason=create-failed");
        return None;
    }
    Some((store, surf_fb, id))
}

/// SCHED-X86: the RENDER service — the interactive OS as a scheduled kernel task, and the sole owner
/// of every pixel. Builds its own `Screen`/`TargetPal`/`Console` over the framebuffer the BSP left in
/// `WRITER` (and detached fbcon from), paints the first frame, then blocks on `GUI_CHANNEL_X86` and
/// dispatches each event through the same routing and the same shared `handle_key` the dismantled BSP
/// loop used. Never returns.
///
/// It owns the ~28 MiB cached-RAM back buffer, which is why the handoff block in `kernel_main` must
/// diverge into `run_bsp` BEFORE the BSP builds one of its own: a second shadow OOMs the 48 MiB metal
/// heap (the same budget `fbcon::attach_shadow` is kept off the GUI path for).
///
/// Presentation is one `pal.render()` per received event. That is strictly FEWER presents than the
/// loop it replaces, which rendered on every pass — i.e. ~1 kHz, since it ended each idle pass in
/// `hlt()` and the periodic timer broke it every tick. Blocking on `recv` means an idle render core
/// is off the run queue entirely and `hlt`s.
#[cfg(target_arch = "x86_64")]
fn x86_render_service(cpu: usize) {
    use core::sync::atomic::Ordering;
    use unaos_kernel::pal::GneissPal; // for pal.width()/height()/render()

    // FrameBuffer is Copy: take a handle and release the WRITER lock immediately. All GUI drawing
    // goes to a cached-RAM back buffer; render() flushes only the damaged region.
    let front_fb = *unaos_kernel::video::WRITER.lock();
    let mut screen = unaos_kernel::video::Screen::new(front_fb);

    // SHELLNOTDESK — is the crispy desktop up? Captured ONCE: `desktop_uefi::activate` runs during PCI
    // enumeration, long before this task is spawned, and never releases the latch on the success
    // path, so the answer is stable for the life of the render service.
    //
    // On the crispy desktop the SCENE owns the backdrop — painted into the desktop layer here, before
    // the first present — and the live text shell is kept off the glass (it survives in serial and
    // `TERM_RING` for the Console app, per the facade law). Off the crispy desktop (a pre-takeover
    // boot, a `wc`-off x86 build) the shell is still the desktop and draws exactly as it always did.
    let desktop = desktop_owns_backdrop();
    if desktop {
        screen.paint_desktop_scene();
    }

    // WCSER-REHOME — is this instance a REPLACEMENT for a render task that died with its core?
    //
    // Consumed (`swap`) rather than read, so the flag cannot leak into some later ordinary spawn.
    // A rescue instance differs from the original in exactly one way, and only one: it does NOT mint
    // a shell window. The dead instance's row is still registered in `wm`'s table and its surface
    // memory is still mapped — a parked core frees nothing — so that window keeps compositing its
    // last contents, and minting a second one would put two shell windows on the desktop and pay
    // ~5 MB for the confusion. The trade is named rather than hidden: **the rescued desktop's shell
    // window is a corpse — it composites, and nothing types into it.** Keys still reach ring-3 apps
    // through `user_input_route` and the window system still gets its clicks, which is the whole of
    // what "input works again" has to mean for this recovery to be worth having.
    let rescue = RENDER_RESCUE_X86.swap(false, Ordering::SeqCst);
    // WCSER-REHOME — this instance's OWNER EPOCH, captured once. Re-checked every pass below; see
    // `RENDER_ROLE_EPOCH_X86` for why a role needs the same revenant discipline `comp_gate_release`
    // gives the gate.
    let my_epoch = RENDER_ROLE_EPOCH_X86.load(Ordering::Acquire);
    if rescue {
        serial_println!(
            ":: SCHED-X86: render task is a REHOMED instance on core {} — draining GUI_CHANNEL_X86 \
             again; the previous instance's shell window is a corpse and is not re-minted ::",
            cpu
        );
    }

    let mut pal = unaos_kernel::pal::TargetPal::new(&mut screen);
    let mut console = unaos_kernel::console::Console::new();

    // SHELLWIN — the live shell's OWN compositor window (crispy desktop only). Built here so its
    // `Screen`/`TargetPal`/`Console` live for the service's life alongside the panel's `pal`, and for
    // the same reason: a persistent `TargetPal` borrows its `Screen`, so both are flat locals (a per-
    // keystroke `TargetPal::new` would also spam the once-per-surface `:: UI1:` line). Off the crispy
    // desktop (pre-takeover, or `open_shell_window` declining) `shell_id` stays `WIN_NONE` and the
    // dummy surface is never drawn — the keystroke path falls to the backdrop `console` exactly as
    // before. Folded to nothing on `wc`-off x86 and (via the enclosing fn's `cfg`) on aarch64.
    // `_shell_store` is bound but never read ON PURPOSE: the `wm` row holds a raw pointer into
    // its heap buffer, and keeping it as a live local (the render service never returns) keeps that
    // allocation from being freed for the row's life. Underscore-prefixed so it raises no unused
    // warning; the empty `Vec` on the non-window paths costs nothing. Every binding is `mut` since
    // SHELLPIN: the dock's pinned shell tile can ask this service to REOPEN a closed shell window,
    // which rebinds the whole tuple (see the reopen arm below the drain loop).
    #[cfg(feature = "wc")]
    let (mut _shell_store, mut shell_screen, mut shell_console, mut shell_id) = {
        let empty = || {
            (
                alloc::vec::Vec::new(),
                unaos_kernel::video::Screen::direct(unaos_kernel::video::FrameBuffer::new()),
                unaos_kernel::console::Console::new(),
                unaos_kernel::video::wm::WIN_NONE,
            )
        };
        // WCSER-REHOME — `!rescue`: a replacement instance does not re-mint the dead one's shell
        // window (see the `rescue` latch above). `empty()` is the same declined-surface shape
        // `open_shell_window` already returns on failure, so no path below needs a new case.
        if desktop && !rescue {
            let info = front_fb.info();
            match open_shell_window(info.width, info.height) {
                Some((store, fb, id)) => {
                    let mut con = unaos_kernel::console::Console::new();
                    con.mark_in_window();
                    // SHELLWIN-OOM — `direct`, NOT `new`: `Screen::new` double-buffers, and its
                    // infallible `vec![0u8; len]` of a second surface-sized (~5 MB) back buffer is
                    // the exact allocation that OOM-panicked GR26's metal boot at desktop-ready,
                    // 14 ms after this window's first present. The surface store above is the one
                    // buffer this window needs; `direct` adds zero.
                    (store, unaos_kernel::video::Screen::direct(fb), con, id)
                }
                None => empty(),
            }
        } else {
            empty()
        }
    };
    #[cfg(feature = "wc")]
    let mut shell_pal = unaos_kernel::pal::TargetPal::new(&mut shell_screen);
    #[cfg(feature = "wc")]
    let mut shell_dirty = false;

    if desktop {
        serial_println!(
            "[shelldesk] backdrop=crispy-scene shell=off-glass bg={:08X} core={}",
            unaos_kernel::video::wm::DESKTOP_BG,
            cpu
        );
        // SHELLWIN — paint the first prompt into the shell window's surface and composite it, so the
        // operator sees a live, focusable shell on the crispy desktop from the first frame.
        #[cfg(feature = "wc")]
        if shell_id != unaos_kernel::video::wm::WIN_NONE {
            shell_console.draw(&mut shell_pal);
            shell_pal.render();
            // SHELLWIN — owner-fenced for consistency with the drain-loop present below (the shell
            // window is a closable `KERNEL_OWNER_DESKTOP` row); harmless here since the row was just
            // created, but it keeps every present of this id on the one recycled-id-safe verb.
            let _ = unaos_kernel::video::wm::present_outcome_owned(
                shell_id,
                unaos_kernel::video::wm::KERNEL_OWNER_DESKTOP,
            );
            serial_println!(
                "[shellwin] backdrop=crispy-scene shell=window win={} surf={}x{} core={} == witness ::",
                shell_id,
                shell_pal.width(),
                shell_pal.height(),
                cpu
            );
        }
    } else {
        console.draw(&mut pal);
    }
    pal.render();
    // The falsifiable pair to the spawn-site line: this one is printed by the task ITSELF, after it
    // has built the panel surface and presented a frame. Spawned is not dispatched (the WINX-2/WINX-3
    // lesson); a spawn line with no dispatch line means the task is sitting in a run queue nobody
    // pops, which is the exact failure this whole arc exists to remove.
    serial_println!(
        ":: SCHED-X86: render task dispatched on core {} — panel owned by the scheduler ::",
        cpu
    );
    // WITCORE: the placement VERDICT, and the reason it lives here rather than at the publish site.
    // `cpu` above is what the spawn site ASKED for; this call additionally reads the core the
    // hardware says it is running on and the split read back out of `smp::SPLIT`, and cross-checks
    // both against the pool `worker_cpu`/`xhci_worker_cpu` will hand out. Three producers, so the
    // PASS/FAIL can actually fail — unlike a check made at publish time against the publisher's own
    // arguments, which is a tautology dressed as evidence.
    //
    // WCSER-REHOME — NOT on a rescue instance, and the skip is a correctness fix rather than a
    // convenience. `confirm_render_core` asks "is this task on the core the boot-time split
    // published?", and for a re-homed render task the honest answer is NO BY CONSTRUCTION: the
    // published core is the corpse we were spawned to replace. Running it would print
    // `verdict=FAIL` for a recovery that worked perfectly, which is worse than not printing — a
    // FAIL that fires on the success path teaches every later reader to ignore the instrument.
    // The re-home prints its own witness (`[wcser] REHOMED …`) naming both cores instead.
    if !rescue {
        unaos_kernel::arch::smp::confirm_render_core(cpu);
    }

    // CURSOR-HIDE: whether the last pass drew the cursor, so the auto-hide transition erases the
    // sprite exactly once. Driven by the input service's `Event::Timer` pulse when nothing is typed.
    let mut cursor_was_visible = false;

    // PTRCH — an event this task has taken OFF the channel and has not dispatched yet: the one
    // non-motion report the fold below had to pull in order to discover that a run of motion had
    // ended. It is dispatched next, in its own turn and in its own order; nothing is ever dropped or
    // reordered by carrying it here.
    //
    // **It lives outside BOTH loops, and that placement is the correctness of the whole fold.** The
    // drain has early exits that are not "the channel is empty" — `handle_key` answering `true` means
    // a command took the whole screen, and the drain stops so the rest of the burst is not painted
    // over it before it is presented. An event held here across one of those exits is off the channel
    // and cannot be re-read from it, so a `pending` scoped to the outer loop's BODY would be
    // re-initialised to `None` on the next pass and the event would be gone — one keystroke or one
    // click swallowed, nondeterministically, only on the boots where a full-screen command ran. Held
    // out here it is instead the first thing the next pass dispatches.
    let mut pending: Option<unaos_kernel::pal::Event> = None;

    loop {
        // WCSER-REHOME — AM I STILL THE INCUMBENT? Two relaxed loads, first thing, every pass.
        //
        // On every ordinary boot this is a compare that never fails: the epoch moves only when a
        // steal declares a core dead. It fires in exactly one situation — this task's core was
        // declared dead, its role was re-homed to a live core, and then this core CAME BACK. That
        // core is a revenant, and if it simply resumed its loop the machine would have two consumers
        // of `GUI_CHANNEL_X86` and two owners of the panel present, which is the split-brain
        // `fbcon::detach()` exists to prevent.
        //
        // It RETIRES rather than returns, and that is load-bearing: `wm`'s rows hold RAW POINTERS
        // into this function's locals (`_shell_store`'s heap buffer, the panel `Screen`), and the
        // whole reason those are flat locals in a never-returning task is to keep them alive for the
        // rows' lifetime. Returning would drop them and leave the compositor blitting from freed
        // memory. So the frame stays on the stack forever and the task sleeps out of the way — off
        // the channel, off the panel, holding nothing.
        if RENDER_ROLE_EPOCH_X86.load(Ordering::Relaxed) != my_epoch {
            serial_println!(
                ":: [wcser] REVENANT render instance on core {} RETIRING — epoch {} was superseded \
                 by {}; this core was declared dead and its role re-homed, and a second consumer of \
                 GUI_CHANNEL_X86 is worse than none == tripwire ::",
                cpu,
                my_epoch,
                RENDER_ROLE_EPOCH_X86.load(Ordering::Relaxed)
            );
            loop {
                unaos_kernel::arch::sched::sleep_ticks(1000);
            }
        }
        // WEDGEINJ (wedgeinj knob, default OFF) — the injected phase-33 park, and it is called HERE,
        // at the top of the render loop, for two reasons. It must run on the RENDER task, because
        // the defect under test is "the core carrying a singleton role died" and killing a pool core
        // would prove nothing. And it must run BEFORE the `recv` below, so the task is already gone
        // when the input service next offers an event — which is the ordering that makes the channel
        // fill and the whole INPUT-UNGATE / WCSER-REHOME chain execute. The `Event::Timer` heartbeat
        // wakes this loop every 250 ms, so the fuse is honoured within one pulse of its deadline even
        // on a desktop with no input at all. Compiles to nothing without the knob.
        unaos_kernel::video::wm::wedgeinj_park_maybe();
        // Block until an event arrives — an idle render core burns nothing. PTRCH: unless one is
        // already in hand, in which case parking would be a deadlock against an event we ourselves
        // removed from the channel.
        let mut raw = match pending.take() {
            Some(ev) => ev,
            None => gui_recv_blocking_x86(),
        };

        // SCHED-X86 DRAIN: dispatch EVERY queued event, then present ONCE below — the dismantled
        // BSP loop's semantic, kept deliberately. Presenting per event is the regression this file
        // already documents (see the CURSOR-10 note on the old loop): a native-resolution flush is
        // slow, so one present per event made "the cursor never catch up; typed text appeared
        // seconds late". A keystroke burst or one trackpad sweep queues dozens of events into the
        // 64-slot channel, and each would otherwise cost its own full present.
        loop {
            // PTRCH — **THE CHANNEL'S OWN FOLD, and the reason PTRDEAD's is not enough on its own.**
            //
            // PTRDEAD folds a relative-motion backlog in `pal::EVENT_QUEUE`. That is the right place
            // for the producer's backlog — but on the SCHED-X86 split the backlog does not sit there.
            // `x86_input_service` runs on its own core and drains `pal::next_event()` to exhaustion
            // every ~1 ms, so the ring is essentially always empty and there is nothing there to fold
            // INTO; what the input service does with each event is offer it into this 64-slot
            // channel, one slot per report. So a render core stalled inside a witness burst
            //
            // INPUT-UNGATE amends the first clause, and only the first: the producer now stops
            // taking from the ring while it holds a must-survive event this channel refused, so
            // "essentially always empty" is true while this task is KEEPING UP and false while it is
            // not. That does not weaken the argument for this fold — it strengthens it. The backlog
            // still lands here first (the channel fills before the ring starts holding), and the
            // producer's own fold only ever engages after this one has already been outrun.
            // accumulates the whole gesture HERE, as 64 separate entries, and then walks them one at
            // a time — each costing a `wc_route_event`, a witness line and a `wc_route_tail`. Same
            // defect, one pipe stage downstream, and the ring's fold cannot see it.
            //
            // The fold is PTRDEAD's, in PTRDEAD's shape, for PTRDEAD's reason: relative deltas are
            // ADDITIVE (`cursor::move_rel` adds; `wc_drag_motion` reads the resulting ABSOLUTE
            // position, so a folded run steers a live drag to exactly the same place), so summing a
            // run of them changes neither the cursor's destination nor any window's.
            //
            // Three properties it shares with the ring's, each load-bearing:
            //   * **it never crosses another event.** The run stops at the first non-`Event::Mouse`
            //     the channel hands back, and that event becomes `pending` — dispatched next, in
            //     order. So a `Button` can never end up behind motion that arrived after it, which
            //     is the ordering GHOST-DRAG's cure is built on. In particular the release edge that
            //     DRAGGLIDE hoisted ahead of its own lift arrives here already ahead of it, and this
            //     fold cannot move it back: it stops AT the edge.
            //   * **`MouseAbsolute` is never folded** — absolute positions are not additive, and the
            //     QEMU tablet fixtures that assert an exact resting coordinate drive that variant.
            //   * **the ledger stays exact.** `GUI_RECV_X86` is charged inside the recv helpers, so
            //     every event pulled by the fold is counted the moment it leaves the channel and
            //     `sent - recv` still reads as live occupancy. The fold gets its OWN term.
            if let unaos_kernel::pal::Event::Mouse { x, y } = raw {
                let (mut sx, mut sy) = (x, y);
                loop {
                    let next = match pending.take() {
                        Some(e) => Some(e),
                        None => gui_try_recv_x86(),
                    };
                    match next {
                        Some(unaos_kernel::pal::Event::Mouse { x: nx, y: ny }) => {
                            // `saturating_add` for the reason `pal` uses it: the fold is unbounded in
                            // principle, and a wrapped delta would throw the arrow across the panel.
                            sx = sx.saturating_add(nx);
                            sy = sy.saturating_add(ny);
                            GUI_FOLD_X86.fetch_add(1, Ordering::Relaxed);
                        }
                        other => {
                            pending = other;
                            break;
                        }
                    }
                }
                raw = unaos_kernel::pal::Event::Mouse { x: sx, y: sy };
            }
            // WINX-7 / CLICK-X86 — the router pair, in the dismantled loop's exact order. A pointer
            // BUTTON is ADDRESSED before it is DELIVERED: `user_input_route` routes by FOCUS, which is
            // right for a keystroke and wrong for a click (that belongs to the window under the cursor),
            // so `wc_click_route` hit-tests the press first and answers `true` only when it CONSUMED the
            // event. Both return `Event::Unknown` (never `Event::None`) for a consumed event.
            //
            // WC-TAB/x86 — and TAB is judged ahead of both, for the reason the dismantled loop's copy
            // of this block states: it is addressed to the window system rather than to any app, and
            // intercepting it after `user_input_route` would mean the focused app swallows the only
            // exit from its own window. This is the seam the bench media actually runs, so it is the
            // one Boot AH's trap was sprung on.
            // USBDBG-INVERT — the debug view on the seam the bench media actually runs: PRINT, then
            // ROUTE. Keyed on the RAW report (see `usbdebug_event_print`) so a report consumed into a
            // focused window's ring is still seen by the operator, and placed ahead of the router so
            // nothing about routing changes on a knob build.
            //
            // PTRCH — the call MOVED from here into [`gui_try_recv_x86`]/[`gui_recv_blocking_x86`],
            // i.e. onto the moment an event leaves the CHANNEL rather than the moment this loop
            // dispatches it. That is the honest seam once the fold above exists: printing here would
            // show one line for a run of reports the hardware genuinely sent, and the operator would
            // read a pad that had gone quiet off a capture in which it was streaming. Raw is what the
            // hardware sent — the fold is this loop's business, not the witness's — and the print is
            // still ahead of every router, still consumes nothing, and still keys on the raw report.
            let ev = unaos_kernel::arch::x86_64::syscall::wc_route_event(raw);

            match ev {
                unaos_kernel::pal::Event::Key(c) => {
                    // INSTGUI — while the installer dialog is open it owns the keyboard; the console
                    // resumes the moment it closes.
                    #[cfg(all(feature = "wc", feature = "instgui"))]
                    let consumed = unaos_kernel::video::instgui::consume_key(c);
                    #[cfg(not(all(feature = "wc", feature = "instgui")))]
                    let consumed = false;
                    // SHELLWIN — route the keystroke to its home. A key reaches this arm only when
                    // `wc_route_event` did NOT hand it to a focused ring-3 window (that app has no key
                    // here) and the installer dialog did not consume it — i.e. the keyboard belongs to
                    // the SHELL (`user_input_active() == 0`). On the crispy desktop the shell is now a
                    // compositor WINDOW, so the key is dispatched into that window's own console and
                    // surface; the backdrop scene is never typed on. Off the crispy desktop (pre-
                    // takeover, wc-off, aarch64) the shell is still the desktop layer and `handle_key`
                    // drives the backdrop `console` exactly as before. `handle_key` answers `true` when
                    // the command took the whole screen — the BSP loop used that to stop DRAINING, and
                    // so do we, so the rest of the burst is not painted over it before it is presented.
                    if !consumed {
                        #[cfg(feature = "wc")]
                        {
                            if desktop {
                                // On the crispy desktop the key never falls to the backdrop console
                                // (the shell is off the glass); it goes to the shell WINDOW, or is
                                // dropped if the window failed to open (logged at bring-up).
                                if shell_id != unaos_kernel::video::wm::WIN_NONE {
                                    let took = handle_key(c, &mut shell_console, &mut shell_pal);
                                    shell_dirty = true;
                                    if took {
                                        break;
                                    }
                                }
                            } else if handle_key(c, &mut console, &mut pal) {
                                break;
                            }
                        }
                        #[cfg(not(feature = "wc"))]
                        {
                            if handle_key(c, &mut console, &mut pal) {
                                break;
                            }
                        }
                    }
                }
                unaos_kernel::pal::Event::Mouse { x, y } => {
                    // CURSOR-X86/CURSOR-WCR: on this target these verbs drive the COMPOSITOR SPRITE in
                    // the front buffer, so `move_rel` repaints the arrow on the report itself and
                    // `draw_over` is the idempotent tail; the leading `restore` the back-buffer targets
                    // need is deliberately absent (it was a duplicated undraw with a wasted publish).
                    unaos_kernel::pal::cursor::move_rel(x, y, pal.width() as i32, pal.height() as i32);
                    unaos_kernel::pal::cursor::draw_over(&mut pal);
                }
                unaos_kernel::pal::Event::MouseAbsolute { x, y } => {
                    unaos_kernel::pal::cursor::set_abs(x, y, pal.width() as i32, pal.height() as i32);
                    unaos_kernel::pal::cursor::draw_over(&mut pal);
                }
                // CLICK-X86 — the PRESS arm. Reaching it means the press was NOT consumed by the click
                // router and NOT taken by a user ring, i.e. it is the SHELL's click, and the x86 shell has
                // no click model yet. Deliberately empty of policy (the disposition is already on the wire
                // from the router's `[clickroute]` line); this is where one attaches when it grows one.
                unaos_kernel::pal::Event::Button(_mask) => {}
                // Timer (the pulse) / KeyUp / Unknown: nothing to dispatch — the tail below still runs.
                _ => {}
            }

            // WMDIRECT — **STEER A LIVE TITLE-BAR DRAG, OFF `raw`, AFTER THE ARMS ABOVE.**
            //
            // The predicate is the RAW report and NOT `ev`, and that distinction is the whole of this
            // line's correctness. `Event::Mouse`/`Event::MouseAbsolute` are PACKABLE, so whenever a
            // ring-3 app holds focus and its ring is not full, `user_input_route` consumes the report
            // and hands back `Event::Unknown` — it never reaches the pointer arms at all. A title-bar
            // grab GUARANTEES exactly that state, because the chrome arm of `wc_click_route_at` calls
            // `user_input_set_active(owner)` with the dragged window's own owner. Keyed on `ev`, the
            // drag would therefore be dead for every app window and alive only for the console and
            // the focus-exempt desktop row (whose arms take `set_active(0)`) — app-dependent,
            // nondeterministic (an app that stops draining fills its ring, the push fails, and the
            // drag springs to life mid-gesture), and with the release edge's unthrottled final
            // reposition TELEPORTING the window to wherever the hand let go.
            //
            // Placed after the `match` rather than beside the routers so that the cursor is FRESH on
            // both branches, which is the other half of it — `wc_drag_motion` reads the shared
            // `pal::cursor` position rather than the report's delta:
            //   * CONSUMED — `user_input_route` -> `pal::cursor::track_routed` has already applied it
            //     (CURSOR-VUG). Without that arc this line would steer to a stale position.
            //   * DECLINED — the `Mouse`/`MouseAbsolute` arms above have just applied it.
            // Exactly ONE tick per pointer report either way, so a relative report is never applied
            // twice and the 16 ms throttle measures real time rather than a doubled rate.
            //
            // Costs one `matches!` on non-pointer events and one atomic load when no drag is live,
            // which is every report on a boot where nobody grabbed a title bar.
            unaos_kernel::arch::x86_64::syscall::wc_route_tail(raw);

            // Take the next queued event if one is already waiting; otherwise the burst is drained
            // and we fall through to the single present. Never parks, so an empty channel costs one
            // failed semaphore try rather than a deschedule.
            //
            // PTRCH — `pending` FIRST: an event the fold above pulled out of the channel to discover
            // the end of a motion run is already off the channel and already counted, so taking it
            // from here is what keeps the drain in arrival order.
            match pending.take().or_else(gui_try_recv_x86) {
                Some(next) => raw = next,
                None => break,
            }
        }

        // SHELLWIN-REOPEN — service the dock's pinned shell tile (SHELLPIN, `video/dock.rs`). The
        // press that latched the request was routed INSIDE the drain above (`wc_route_event` ->
        // `wc_click_route_at` -> `dock::press_at`), so by this line the flag is set and the reopen
        // lands in the same burst — no extra wakeup, no new event variant, the channel untouched.
        //
        // Two arms, one live shell window max:
        //  * the row is still alive under its owner (the scan-to-press race, or a parked shell) —
        //    RAISE it through the same pair the click router's furniture arm uses, never a second
        //    window. `focus_changed` also un-parks (fresh top-of-stack z), so a minimised shell
        //    comes back through this arm too.
        //  * the row is gone (the operator closed their only shell) — rebuild it through the SAME
        //    fallible path bring-up used: `open_shell_window` (`try_reserve_exact` decline ->
        //    `[shellwin] DECLINE`) and `Screen::direct` (NEVER `Screen::new` for a window surface —
        //    its infallible second back buffer is the exact allocation that OOM-panicked GR26's
        //    metal boot). The whole tuple is rebound: the old store's Vec is freed by the
        //    assignment (its row is already reaped, so no raw pointer outlives it), the new
        //    `TargetPal` re-borrows the new `Screen` (one fresh `:: UI1:` line per reopen, the
        //    once-per-surface rule kept), and `shell_id` now routes keystrokes to the NEW window.
        //    `user_input_set_active(0)` hands the keyboard back so the first keystroke after the
        //    reopen lands in the reopened shell.
        // A DECLINE leaves the old (dead) id in place: presents stay fenced off by
        // `present_outcome_owned` and the pinned tile stays on the dock for another try.
        #[cfg(feature = "wc")]
        if desktop && unaos_kernel::video::dock::take_shell_reopen() {
            use unaos_kernel::video::wm;
            if shell_id != wm::WIN_NONE && wm::owner_of(shell_id) == Some(wm::KERNEL_OWNER_DESKTOP) {
                unaos_kernel::arch::x86_64::syscall::user_input_set_active(0);
                wm::focus_changed(wm::KERNEL_OWNER_DESKTOP);
                serial_println!("[shellwin] reopen route=dock already-live win={} raised", shell_id);
            } else {
                let info = front_fb.info();
                match open_shell_window(info.width, info.height) {
                    Some((store, fb, id)) => {
                        _shell_store = store;
                        shell_console = unaos_kernel::console::Console::new();
                        shell_console.mark_in_window();
                        shell_screen = unaos_kernel::video::Screen::direct(fb);
                        shell_pal = unaos_kernel::pal::TargetPal::new(&mut shell_screen);
                        shell_id = id;
                        // First frame, exactly as bring-up: prompt into the surface, flush, one
                        // owner-fenced present so the reopened shell is on glass before the next
                        // event arrives.
                        shell_console.draw(&mut shell_pal);
                        shell_pal.render();
                        let _ = unaos_kernel::video::wm::present_outcome_owned(
                            id,
                            unaos_kernel::video::wm::KERNEL_OWNER_DESKTOP,
                        );
                        unaos_kernel::arch::x86_64::syscall::user_input_set_active(0);
                        shell_dirty = false;
                        serial_println!("[shellwin] reopen win={} route=dock == witness ::", id);
                    }
                    // The decline reason (`alloc`/`create-failed`) is already on the wire from
                    // `open_shell_window`; nothing was torn down, nothing to roll back.
                    None => {}
                }
            }
        }

        // CURSOR-HIDE: restore the pixels under the sprite once when the auto-hide delay expires
        // (reappearance is instant — the pointer arms above stamp the activity clock before drawing).
        let cursor_vis = unaos_kernel::pal::cursor::visible();
        if cursor_was_visible && !cursor_vis {
            unaos_kernel::pal::cursor::restore(&mut pal);
        }
        cursor_was_visible = cursor_vis;

        // INSTGUI: pick up disks that enumerate after the dialog opened (repaints only on change).
        // Rides the pulse, which is why the pulse exists.
        #[cfg(all(feature = "wc", feature = "instgui"))]
        unaos_kernel::video::instgui::service();

        // Present: flush the damaged region of the back buffer to the framebuffer. A no-op when
        // nothing was drawn, so a pure cursor pass (front-buffer sprite) costs almost nothing.
        pal.render();

        // SHELLWIN — flush the shell window's surface and composite it ONCE per drained burst, the same
        // drain-then-present-once discipline the backdrop above follows: `handle_key` drew into the
        // shell's cached-RAM back buffer as keys arrived, `render()` copies the damaged rows into the
        // window surface, and `wm::present` composites that surface over the crispy backdrop (the panel
        // flush above already subtracted the window's box from its own damage, so nothing painted over
        // it). After the backdrop present, so the shell window lands on top of the scene.
        #[cfg(feature = "wc")]
        if shell_dirty {
            shell_pal.render();
            // SHELLWIN — the OWNER-FENCED present, not the fence-free `present`. The shell window is
            // `KERNEL_OWNER_DESKTOP`, and `wm::controls`/`ctrls_for` give every non-compat, non-owner-0
            // row a live close/minimise/zoom cluster — so an operator CAN close it (the kernel-furniture
            // close arm reaps the row by id) or minimise it (parked below `SHELL_Z`). `shell_id` is a
            // recycled slot alias with no generation, so a fence-free `present(shell_id)` after a close
            // would composite whatever row later took the slot. `present_outcome_owned` declines a slot
            // whose owner is no longer `KERNEL_OWNER_DESKTOP` (`NoRow`) and suppresses a parked one
            // (`Suppressed`) — the same recycled-id fence `fbcon` uses for the closable console window
            // (NORMALWIN). The close is no longer a one-way trip: the dock PINS a permanent shell
            // tile (SHELLPIN, `video/dock.rs`) whose press latches a reopen request this loop
            // services below — the reopen route the GR27 review note asked for, per the standing
            // rule ("closeable means build the reopen route, not withhold the button").
            let _ = unaos_kernel::video::wm::present_outcome_owned(
                shell_id,
                unaos_kernel::video::wm::KERNEL_OWNER_DESKTOP,
            );
            shell_dirty = false;
        }

        // SCHED-X86 depth witness. `sent - recv` is the LIVE occupancy of the 64-slot channel, and it
        // is the number that separates "the render task is keeping up" from "the render task is
        // wedged and the input task is one burst away from blocking in `send`". Both counters are
        // read HERE, from the consumer side, after the present — so the line is a measurement of the
        // steady state, not of this task's own start-up. Rate-limited to ~5 s.
        let now_ms = unaos_kernel::arch::ms();
        let last = GUI_DEPTH_LAST_MS.load(Ordering::Relaxed);
        if now_ms.wrapping_sub(last) >= 5000 || last == 0 {
            GUI_DEPTH_LAST_MS.store(now_ms.max(1), Ordering::Relaxed);
            let sent = GUI_SENT_X86.load(Ordering::Relaxed);
            let recv = GUI_RECV_X86.load(Ordering::Relaxed);
            // PTRCH — `fold` is APPENDED at the tail (the standing insertion rule), and it is the term
            // that separates the two ways `inflight=0` can be true. A channel that is empty because
            // nothing was queued and a channel that is empty because the consumer summed a whole
            // gesture into one dispatch read identically in `sent`/`recv`/`inflight`; `fold` is how
            // many reports took the second path, i.e. how far behind the pad this core fell.
            serial_println!(
                "[schedx86] depth sent={} recv={} inflight={} (render core {}) fold={}",
                sent,
                recv,
                sent.wrapping_sub(recv),
                cpu,
                GUI_FOLD_X86.load(Ordering::Relaxed)
            );
            // SCHEDLOAD-X86 load witness, riding the depth line's clock gate — the two are the answer
            // halves of one question and are worth reading as a pair: `depth` says whether the GUI
            // pipe is backed up, `load` says what the other seven cores were doing while it was not.
            // Every boot emits this without an operator launching anything, which is the whole point:
            // until now the only per-core load feed on x86 was `SYS_CPUPULSE`, i.e. a ring-3 app
            // somebody had to start, so no unattended capture has ever contained a load number.
            //
            // Emitted from HERE — after `pal.render()`, inside the existing rate limit — deliberately.
            // The event-routing block above (`wc_click_route` -> `user_input_route` -> `handle_key`)
            // is the seam of the open focus-trap defect and is not to be perturbed by an instrument.
            unaos_kernel::arch::sched::emit_load_witness("");
            // R0 / rtwit: the WORST-CASE RULER's rollup, riding the same ~5 s gate. Emits the
            // `[rtwit]` line (input→present max/p99, per-lock max holds, max interrupt-mask span,
            // ruler overhead) and resets every per-span slot. A no-op inline shim when `rtwit` is off.
            unaos_kernel::rtwit::rollup();
            // R1 / rtpi: the PRIORITY-INHERITANCE witness rollup, riding the same ~5 s gate. Emits
            // `[rtpi] inherits=<n> max_jump=<lvl> chain_max=<d> active=<gauge>` (0 / `--` honestly
            // when no inversion occurred) plus the span's rate-limited `[rtpi] inherit …` traces.
            // `#[cfg]`-gated (not a shim call) so a knob-off kernel carries none of its symbols and
            // stays bit-identical.
            #[cfg(feature = "rtpi")]
            unaos_kernel::rtpi::rollup();
        }
    }
}

// ── SERWIT-1 ────────────────────────────────────────────────────────────────────────────────────
// Drive the serial-transport stress fixture: one kernel worker per online AP, all parked on a gate so
// their bursts overlap instead of trickling one core at a time (a trickle would never contend, and a
// fixture that cannot reproduce the defect it guards is decoration). This runs on the boot path,
// before the BSP joins the scheduler, so it waits the same bounded-on-`ticks()` way `await_verdict`
// does rather than joining, then prints the verdict itself. Bounded on both ends: the wait gives up on
// the ms-clock, and the verdict prints the real numbers either way — a timeout shows up as a short
// `sent` count and reads FAIL, never as silence, which would be an ironic way for this particular
// fixture to fail.
// ── SERWIT-1 ────────────────────────────────────────────────────────────────────────────────────
// Drive the serial-transport stress fixture: one kernel worker per online AP, all parked on a gate so
// their bursts overlap instead of trickling one core at a time (a trickle would never contend, and a
// fixture that cannot reproduce the defect it guards is decoration). The BSP is not scheduled, so it
// waits the same bounded-on-`ticks()` way `await_verdict` does rather than joining, then prints the
// verdict itself. Bounded on both ends: the wait gives up on the ms-clock, and the verdict prints the
// real numbers either way — a timeout shows up as a short `sent` count and reads FAIL, never as
// silence, which would be an ironic way for this particular fixture to fail.
#[cfg(all(target_arch = "x86_64", feature = "witness"))]
fn serwit1_run(online: &[usize]) {
    let workers = unaos_kernel::serial_ring::serwit_worker_count(online.len());
    let (b_sub, b_emit, b_drop) = unaos_kernel::serial_ring::serwit_snapshot();
    for (i, &cpu) in online.iter().take(workers).enumerate() {
        let _ = i;
        unaos_kernel::arch::sched::spawn("serwit-burst", serwit_entry, cpu, cpu, 1);
    }
    unaos_kernel::serial_ring::serwit_release();
    let deadline = unaos_kernel::arch::ticks() + 4000; // ~4 s at the calibrated 1 kHz
    while !unaos_kernel::serial_ring::serwit_all_done(workers)
        && unaos_kernel::arch::ticks() < deadline
    {
        core::hint::spin_loop();
    }
    unaos_kernel::serial_ring::serwit_verdict(workers, b_sub, b_emit, b_drop);
}

/// `spawn` entry thunk: the scheduler hands the task its `arg`, which here is the core index the
/// worker stamps into every line it prints.
#[cfg(all(target_arch = "x86_64", feature = "witness"))]
fn serwit_entry(cpu: usize) {
    unaos_kernel::serial_ring::serwit_worker(cpu);
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // SERWIT-1: switch the serial transport to its raw, lock-free, synchronous mode BEFORE the first
    // print below. From here on `_print` does not touch the UART Mutex at all — which is what makes
    // these two lines reach the wire even when this very core died holding it (the old shape lost the
    // `try_lock` to itself and dropped the whole panic message: red screen, no words). It also flushes
    // anything other cores had staged just before the fault. Takes no lock, so it cannot deadlock.
    unaos_kernel::serial_ring::enter_panic_mode();
    // Paint a red panic backdrop on the framebuffer (visible on hardware with no serial), then
    // print the message — serial_println! mirrors it onto that backdrop via fbcon.
    unaos_kernel::video::fbcon::panic_screen();
    serial_println!("=== KERNEL PANIC ===");
    serial_println!("{}", info);
    unaos_kernel::arch::hlt_loop();
}

// ── RAST-TEGRA ──────────────────────────────────────────────────────────────────────────────────
// The Orin-panel wire-in of the `rast` software rasterizer. Called from the tail of `tegra_early_stop`
// (post-drop, at EL1, right before `run_capstone_boot_core`) so the spinning cube draws through the
// JD1-inherited scanout as the last panel content of the boot. Kept HERE at the file tail — and called
// on the SAME source line as the terminus above — so the whole wire-in adds zero source lines ahead of
// any panic `Location` literal: the tegra knob-off kernel is byte-identical to baseline (the panic-line
// byte-identity constraint, PI-V3D-1 bisect-proven). Same `Screen` present path RAST-1 proved on x86,
// call-never-edit on the shared surface. Panel comes off `video::WRITER` (seeded by JD1 in
// `tegra_early_stop`, mapped into BOTH translation tables so it is reachable post-drop at EL1); the
// back buffer and depth buffer come off the live 48 MiB heap. `crate::arch::ms()` reads CNTVCT on the
// timerless post-drop core (the VUGFIX tegra fallback), so the honest fps line still ticks.
#[cfg(all(feature = "tegra", feature = "rast", target_arch = "aarch64"))]
fn tegra_rast_demo_maybe() {
    let front_fb = *unaos_kernel::video::WRITER.lock();
    // Headless boot (no JD1 scanout → WRITER never seeded): nothing to draw on, stay silent-ish.
    if front_fb.info().width == 0 {
        serial_println!(":: RAST: tegra headless (no JD1 scanout) — cube demo skipped ::");
        return;
    }
    // Detach fbcon's serial mirror first so a CAPSTONE straggler line can't paint over the demo
    // frames (serial output is unaffected). Mirrors the x86/virt wire-in and the JD2 phase-2 takeover.
    if !tegra_conwin_live() { unaos_kernel::video::fbcon::detach(); } // ORIN-CONWIN rung 4 — THE SAME GUARD ITS TWIN AT `jd2_console_pump`'s phase-2 line already carries, and it was missing here. THE DEFECT IT REMOVES: on the terminus line `orin_conwin` runs BEFORE `tegra_rast_demo_maybe`, so every image carrying both (all five orinconwin legs carry `rast`) installed the console-window route, printed `[orinconwin] … live=LIVE`, and then reached THIS statement and detached anyway — `GUI_ACTIVE` set, `fbcon::_print` returning at its first test, and the "LIVE" console window receiving no further glyph for the rest of the boot. The guard is the same one and for the same reason: `tegra_conwin_live()` answers true only when `fbcon::console_is_routed()` does, and a routed console does not write the PANEL — `FbCon::draw_fb` hands back `win_fb`, kernel RAM no scan-out reads — so the single-writer guarantee the detach exists for is already true of the mirror path. ⚠ WHAT THE GUARD COSTS HERE, and it is NOT identical to the phase-2 site's cost, so it is stated rather than inherited: this site's stated purpose is "a straggler can't paint over the DEMO FRAMES", and a routed console still reaches glass through `wm`'s paced, damage-limited composite. So a straggler line printed while the cube spins can now composite over it INSIDE the console window's rect, and because `orin_rast_console_owns()` is not latched until phase 2 the census would score that `RAST-PAINTED-OVERWRITTEN`. That is a true reading of a real second writer, not a false one — the conwin+rast image genuinely has two panel owners — and it is the same trade the phase-2 line already took when it chose a live console over a frozen one. ⚠ FOLDED IN PLACE, never added lines: panic `Location` records embed line numbers and the knob-off jetson image's byte-identity is this track's standing proof. Knob-off `tegra_conwin_live` is `#[inline(always)] false`, so this folds to the bare `detach();` it has always been.
    serial_println!(":: RAST: tegra — first 3D pixels on the Orin panel (inherited scanout) ::");
    let mut screen = unaos_kernel::video::Screen::new(front_fb);
    unaos_kernel::rast_demo::run_mc(&mut screen); unaos_kernel::rast_demo::run(&mut screen); drop(screen); unaos_kernel::arch::display_tegra::orin_rast_glass_post(); // RAST-MC: the multi-core rung runs FIRST (1-core baseline + frame-pipelined pass, both unpaced) so the paced visible spin stays the last panel content of the boot; same-line per the zero-added-lines convention above. ORIN-RASTGLASS: the `post` glass read-back, fired the instant `run` returns — nothing is dispatched on this core in between, so it is the ONE sample that can establish whether the blit reached the scan-out at all (`late`, from the pump sweep, then says whether it survived). `drop(screen)` first so the double buffer's own `flush` has certainly landed before the panel is read back; also releases the back buffer to the heap ahead of ~2048 volatile VRAM reads. Same-line per the convention above.
}

// Knob-off / non-rast tegra build: the wire-in compiles to nothing. `#[inline(always)]` on an empty
// body means the call above emits zero instructions, so the tegra image stays byte-identical.
#[cfg(all(feature = "tegra", not(feature = "rast"), target_arch = "aarch64"))]
#[inline(always)]
fn tegra_rast_demo_maybe() {}

// ── JETSON-EL0 (M1b): the Orin's first EL0 round trip ───────────────────────────────────────────────
//
// Bring the user address space up, load the `USER_BLOB` hello program into it, seal the code page W^X,
// and enqueue one EL0 task plus its verdict — the `run_capstone_boot_core` terminus dispatches both
// (a spawn only pushes to the run queue).
//
// WHY IT LIVES AT THE FILE TAIL, CALLED ON THE TERMINUS LINE. Exactly the RAST-TEGRA convention above,
// for exactly the RAST-TEGRA reason (PI-V3D-1, bisect-proven): `panic!`/`assert!`/bounds-check sites
// bake a `core::panic::Location` — file AND LINE — into rodata, so inserting even a comment line ahead
// of them shifts every later Location and the knob-off image stops being byte-identical. A tail
// definition plus a same-line call adds ZERO source lines before any Location, which is what keeps the
// disarmed jetson media bit-for-bit equal to baseline. (Found the hard way: the first cut of this arc
// put a 35-line block inline here and moved the media hash with the knob OFF.)
//
// WHY IT SITS AFTER `timer::set_not_live()` — a deviation from the M1b brief, recorded because it is
// load-bearing. The brief put the spawn BEFORE that disarm. It cannot go there: the user window hangs
// off `L1_EL1`, and `L1_EL1` is not the live root until `boot_tegra::drop_to_el1` installs it in
// TTBR0_EL1, so a pre-drop `syscall::setup()` would copy the blob through a user VA that translates to
// NOTHING under the EL2 `L1` and fault into the Part-C vector. `set_not_live()` is on the very next
// line after that drop, so "after the drop" is necessarily "after the disarm". Nothing is lost: the
// disarm retires the timer INTERRUPT (CNTP_CTL=0 at the drop), while the verdict's bound is CNTPCT,
// which free-runs. The site is also after `exceptions::install()` on purpose — that is what puts the
// real `VBAR_EL1` in place for the `SVC` the EL0 task takes; `mmu_tegra::arm_el1_fault_vector()` is a
// syndrome-printing landing vector, not a syscall handler.
#[cfg(all(feature = "tegra_el0", target_arch = "aarch64"))]
fn tegra_el0_start_maybe() {
    if !unaos_kernel::arch::mmu_tegra_el0::install() {
        // `install` already printed WHY it refused; this is the machine-checkable FAIL line, so a
        // capture never has to infer the outcome from the absence of a PASS.
        serial_println!(":: TEGRA-EL0: el0-hello round-trip -> FAIL — user window not installed ::");
        return;
    }
    let demo = unaos_kernel::arch::syscall::setup();
    unaos_kernel::arch::syscall::protect();
    // Pinned to the boot core (cpu 0, not CPU_AUTO): `pick_cpu_slot` short-circuits a non-AUTO request,
    // so the EL0 task cannot be placed on a secondary. That pin is load-bearing — the boot core is the
    // one this function has just proven is at EL1 with the real `VBAR_EL1` installed.
    unaos_kernel::arch::sched::spawn_user("el0-hello", demo.hello, demo.sp, 0);
    unaos_kernel::arch::sched::spawn(
        "tegra-el0-verdict",
        unaos_kernel::arch::syscall::tegra_el0_verdict,
        0,
        0,
    );
    serial_println!(
        ":: TEGRA-EL0: el0-hello spawned at EL0 (boot core), verdict armed — entry {:#x} sp {:#x} ::",
        demo.hello,
        demo.sp
    );
}

// Knob-off tegra build: the wire-in compiles to nothing. `#[inline(always)]` on an empty body means the
// call on the terminus line emits zero instructions, so the tegra image stays byte-identical.
#[cfg(all(feature = "tegra", not(feature = "tegra_el0"), target_arch = "aarch64"))]
#[inline(always)]
fn tegra_el0_start_maybe() {}

/// DARKWIN-GUARD witness (orin 1, 2026-08-18) — tail-defined per the Location-shift convention
/// (`main.rs` "35-line block" lesson: inserting lines ahead of panic/assert sites shifts their
/// baked `core::panic::Location` and moves the knob-off media hash; tail definitions + same-line
/// calls shift nothing). Called from `tegra_early_stop` right after the `mmu live` line.
///
/// Nonzero N means some caller printed before `mmu_tegra::init` — the exact class (EDID-CARRY's
/// step-0a2 witness line) that stopped the merged base at the NVIDIA logo on every boot of
/// 2026-08-18. The dropped bytes' content is gone by design (the count is the wire-side fact;
/// fbcon/selftest mirrors still carried the text); re-emit the one witness we know the window
/// ate, so the EDID reading still reaches the wire on tegra.
#[cfg(all(feature = "tegra", target_arch = "aarch64"))]
fn tegra_darkwin_witness(boot_info: &BootInfo) {
    serial_println!(
        ":: tegra: dark-window guard — {} byte(s) dropped pre-map ::",
        unaos_kernel::arch::serial::dropped_pre_map()
    );
    unaos_kernel::video::init_edid(
        &boot_info.edid_block,
        boot_info.edid_block_valid,
        boot_info.edid_total_len,
    );
}

// ── PI-RAST ─────────────────────────────────────────────────────────────────────────────────────
// The Pi 4 / BCM2711 panel wire-in of the `rast` software rasterizer — the Pi's first 3D pixels.
// RAST-TEGRA is the precedent this follows: an aarch64 board with an INHERITED scanout. There is no
// mode-set and no scanout reprogramming here either. The panel is whatever the VideoCore firmware
// already gave us through the mailbox `init_framebuffer` (`video::WRITER`), and geometry is read
// LIVE off that surface — never hardcoded. The bench Pi is 1920x1200 and QEMU raspi4b is 640x480;
// both are just `screen.width()/height()` to this code, and `rast_demo::run` centers its fixed
// 320x240 render on whatever it finds (and skips honestly if the panel is smaller than that).
//
// WHERE IT RIDES, and why that point and not the terminus. On aarch64/baremetal the boot core reaches
// the GUI handoff block, detaches fbcon, spawns the `input` + `render` service tasks onto two APs and
// then joins the scheduler (`run_bsp`). The demo is called on the DETACH LINE — after the panel is up
// and fbcon has stopped mirroring serial onto it, and BEFORE any service task exists. So:
//   * it cannot race bring-up (the mailbox framebuffer, the heap and the timer are all long up), and
//   * it cannot fight the compositor (`render_service` is the panel's sole painter, and it has not
//     been spawned yet — unlike the Orin, whose terminus IS the scheduler entry).
// The demo's own `Screen` (a full-panel back buffer, ~9 MiB at 1920x1200) is scoped to this call and
// DROPPED before those spawns, so the render task's identical shadow never coexists with it on the
// 48 MiB metal heap. `rast_demo::run` is bounded (90 paced frames), so boot always reaches the shell;
// `render_service` repaints the console over the cube the moment it starts.
//
// CALL-NEVER-EDIT on the shared surface: this touches only the public `Screen` API through
// `rast_demo::run`, exactly as x86/virt and tegra do. It does not touch the compositor, `pal`,
// `pal::cursor::SPRITE_OWNS_PAINT` (which stays false on aarch64), or anything in the V3D tree.
// The helper lives HERE at the file tail and is CALLED on an existing source line, so the whole
// wire-in adds zero source lines ahead of any panic `Location` literal — the constraint that keeps
// the knob-off kernel8.img byte-identical to baseline.
#[cfg(all(feature = "pi", feature = "pirast", target_arch = "aarch64"))]
fn pi_rast_demo_maybe() {
    // `FrameBuffer` is `Copy` — take a handle and release the WRITER lock immediately, the same way
    // `render_service` does a few dozen lines below.
    let front_fb = *unaos_kernel::video::WRITER.lock();
    // Headless / no firmware framebuffer: nothing to draw on. Say so and return — never guess a size.
    if front_fb.info().width == 0 || front_fb.info().height == 0 {
        serial_println!(":: PI-RAST: no mailbox framebuffer (headless boot) — cube demo skipped ::");
        return;
    }
    let mut screen = unaos_kernel::video::Screen::new(front_fb);
    serial_println!(
        ":: PI-RAST: BCM2711 mailbox panel {}x{} (live firmware geometry, inherited scanout) — software rasterizer cube, the Pi's first 3D pixels ::",
        screen.width(),
        screen.height()
    );
    let t0 = unaos_kernel::arch::ms();
    unaos_kernel::rast_demo::run(&mut screen);
    // Honest fps for the WHOLE Pi wire-in: measured wall clock across `Screen` construction, the
    // render/present loop and its pacing — a strictly wider span than the `:: RAST:` line the shared
    // demo prints for the loop alone, so the two are expected to differ slightly and both are real.
    // `PI_RAST_FRAMES` mirrors `rast_demo::FRAMES` (see the constant's own note on keeping it true).
    let elapsed = unaos_kernel::arch::ms().saturating_sub(t0).max(1);
    let fps_x1000 = (PI_RAST_FRAMES as u64 * 1000 * 1000) / elapsed;
    serial_println!(
        ":: PI-RAST: {} frames in {} ms — {}.{:03} fps (software rasterizer, BCM2711 mailbox-fb present) ::",
        PI_RAST_FRAMES,
        elapsed,
        fps_x1000 / 1000,
        fps_x1000 % 1000
    );
}

/// Frame count of the shared `rast_demo` loop, mirrored here so the PI-RAST fps line can report a
/// rate rather than a bare duration without making `rast_demo`'s internals public. If the shared
/// demo ever changes `FRAMES`, this constant must move with it — the `:: RAST:` line the shared demo
/// prints one line earlier carries its OWN frame count, so a drift is visible on the wire (the two
/// counts disagreeing in the same log) rather than silent.
#[cfg(all(feature = "pi", feature = "pirast", target_arch = "aarch64"))]
const PI_RAST_FRAMES: u32 = 90;

// Knob-off pi build (the default, and every `./arroyo kernel8` that does not set UNAOS_PIRAST=1):
// the wire-in compiles to nothing. `#[inline(always)]` on an empty body means the call on the detach
// line emits zero instructions, so kernel8.img stays byte-identical to baseline.
#[cfg(all(feature = "pi", not(feature = "pirast"), target_arch = "aarch64"))]
#[inline(always)]
fn pi_rast_demo_maybe() {}

// ── CONSWIN-PI / MENUBAR-PI ─────────────────────────────────────────────────────────────────────
// The Pi's DESKTOP-READY wire-in. Called from the aarch64/baremetal GUI handoff, on the SAME source
// line as the `fbcon::detach()` it guards, so the whole thing adds zero source lines ahead of any
// panic `Location` literal — the knob-off byte-identity constraint PI-RAST and PI-V3D-1 established
// on this track and which this arc keeps unbroken in five files.
//
// It GUARDS the detach rather than merely riding it, and that is the substance of the console half of
// the arc. `fbcon::detach()` sets `GUI_ACTIVE`, after which `fbcon::_print` returns at its first test
// and no kernel line reaches the console again by any path — which is why the PI-DESK M4 assessment
// concluded a Pi console window "would frame a frozen log". The premise was right and the conclusion
// does not follow, because the detach's REASON is discharged by the routing: it exists so that exactly
// one core writes the panel once the GUI owns it, and a routed console writes kernel RAM, not the
// panel (`FbCon::draw_fb` returns the window surface; the pixels reach glass only through `wm`'s
// staged present). So when — and only when — the console was successfully routed, the handoff skips
// the detach and the console window stays LIVE for the rest of the boot.
//
// Worth recording that this makes the Pi's console window BETTER than x86's rather than equal to it:
// on x86's desktop lane the same row is, in `desktop_uefi.rs`'s own words, "a FROZEN BOOT-LOG SNAPSHOT for the
// rest of the boot", because that lane detaches unconditionally. The live routed console exists on
// x86 only on the `usbdebug` bench lane, which never detaches at all. Same code, three lanes, and the
// Pi now runs it on the one that matters to an operator.
#[cfg(all(target_arch = "aarch64", feature = "desktop_firmware", feature = "baremetal"))]
fn desktop_firmware_activate_maybe() -> bool {
    unaos_kernel::video::desktop_firmware::activate()
}

// Knob-off ON THE PATH THAT HAS THE CALLER: the wire-in compiles to nothing. `#[inline(always)]` on a
// constant `false` means the call site emits zero instructions and folds to the bare `detach()` it has
// always been. ⚠ Gate NARROWED from `not(all(desktop_firmware, baremetal))` — see DEAD-STUB in the DESKFURN block.
#[cfg(all(target_arch = "aarch64", feature = "baremetal", not(feature = "desktop_firmware")))]
#[inline(always)]
fn desktop_firmware_activate_maybe() -> bool {
    false
}

// =================================================================================================
// ORIN-WM1 (tail block) — why the call site above is ONE APPENDED STATEMENT and where it sits.
// =================================================================================================
//
// THE STATEMENT. `tegra_early_stop`'s step 3c line, `serial_println!(":: KERNEL HEAP ALLOCATED ::");`,
// carries `#[cfg(feature = "orindesk")] unaos_kernel::arch::display_tegra::orin_wm1();` APPENDED to it
// rather than a new line of its own. That is the `smpmark` / DARKWIN-GUARD idiom in this tree, and the
// reason is measured, not stylistic: inserting a line into `main.rs` shifts the line-number constants
// baked into every panic `Location` below it, which moves the loadable image even when not one
// instruction of logic changed. Appending keeps the file's line count identical, which is what lets the
// DISARMED jetson image be byte-identical to baseline rather than merely argued to be.
//
// WHY HERE AND NOT `kernel_main`. `tegra_early_stop` is declared `-> !` and DIVERGES: `kernel_main`'s
// own `memory::init`, its `video::init_panel` (the tree's only `wm::reserve_stage` caller) and every
// later step are unreachable on this board. A desktop seam hung off `kernel_main` would be dead code on
// the Orin. The two live seams are this function and `jd2_console_pump`.
//
// WHY THIS LINE AND NOT `jd2_console_pump`. Four preconditions have to hold at once, and this is the
// first instruction on the tegra path where they do:
//
//   * PANEL — `video::WRITER` carries real geometry. Seeded in the JD1 block ~170 lines above; before
//     it, `spawn_geometry` declines and a row would be born unplaced.
//   * HEAP — `wm::reserve_stage` grows a `Vec` with `try_reserve`. The heap is carved by the
//     `memory::init` two lines above; this statement rides the line that ANNOUNCES it, so the ordering
//     is not merely correct, it is legible.
//   * IRQs LIVE, NO PASS IN FLIGHT — `reserve_stage`'s own stated contract (it must not be the growth
//     that happens inside a present's IRQ mask). JM4's `enable_irq` is ~15 lines above; the scheduler
//     has not started, and this is the boot core alone.
//   * STACK — `composite_inner`'s aarch64 stack cost is on the ledger (occ62 records stack exhaustion
//     as the reason that function was reshaped), and the Pi hit a 16 KiB kernel-stack overflow in the
//     desktop cascade twice. `tegra_early_stop` runs on the BOOT stack; `jd2_console_pump` is a
//     `sched::spawn`ed task on a `TASK_STACK_SIZE` stack. This rung takes the boot stack.
//
// DARKWIN. Every `serial_println!` `orin_wm1` can emit is downstream of `serial::mark_mmio_ready()` —
// which is armed on the MMU line at the very top of `tegra_early_stop`, some 280 lines above — so no
// byte of it can reach an unmapped UARTC.
//
// SCOPE. One window. `desktop_firmware::activate` is NOT called from here and no furniture is armed; see the
// ORIN-WM1 block in `arch/aarch64/display_tegra.rs` for the cascade hazard that keeps it that way.

// =================================================================================================
// ORINDESK RUNG 2 — DESKSEAM: the Orin's DESKTOP SEAM. `tegradesk`, DEFAULT OFF, and it REFUSES
// even when armed. Ladder: docs/dev/OS/08_VIDEO/orin-desktop.md §3.1, §3.2, §5.2, §6, §6.1.
// =================================================================================================
//
// THE DEFECT THIS CLOSES. `desktop_firmware_activate_maybe` above is gated
// `all(aarch64, feature = "desktop_firmware", feature = "baremetal")`. `Cargo.toml` has `baremetal = ["pi"]`
// and `arch/aarch64/serial.rs` has `#[cfg(all(feature = "pi", feature = "tegra"))] compile_error!`,
// so on a tegra build that gate is not merely unset — it is UNSATISFIABLE, and the wrapper always
// resolves to its constant-`false` twin. The desktop-arming code has existed on this branch since
// the base sync and could never once have run here. `tegra_desk_arm` is the gate that CAN be
// satisfied on this board. The Pi wrapper above is UNTOUCHED: it is live and correct on `hw-pi4`,
// and this is an addition beside it, not a widening of it.
//
// WHY THE CALL SITE IS THE TERMINUS LINE (§3.1). `tegra_early_stop` is declared `-> !` and DIVERGES;
// `kernel_main` — where `desktop_firmware_activate_maybe` is wired, behind the `fbcon::detach()` handoff — is
// never reached on this board. §3.1's conclusion is that the arming point "has to be chosen inside
// `tegra_early_stop`, after the heap and the scheduler are up", and the last instruction before
// `run_capstone_boot_core(0)` (which never returns) is the only point on this path where all of it
// holds at once: panel seeded (JD1, ~570 lines above), heap carved (step 3c), IRQs live (JM4), SMP
// secondaries kicked off (JM5, ~95 lines above — `wm::reserve_stage` sizes one stage entry per LIVE
// core, so §3.3's ordering constraint is inherited), EL2 -> EL1 dropped, `percpu`/`mark_el1_core`
// stamped, and the run queue populated but not yet driven. It is also the tegra counterpart of the
// Pi's `main.rs:1316` GUI-handoff line in every respect that matters.
//
// AND IT RUNS ON THE BOOT STACK, which is a better stack story than the Pi's and is stated here
// because §5's hazard is a stack hazard: both Pi overflows were `TASK_STACK_SIZE` (16 KiB) task
// stacks under a preemption frame. `tegra_early_stop` is the boot core's own entry frame. That is
// NOT a claim that the cascade fits — see the stop-line below and the report's NOT-MEASURED list.
//
// ZERO SOURCE LINES. The call site is a statement APPENDED to the existing terminus line and this
// block is at the FILE TAIL, below every panic `Location` in `main.rs`. Inserting a line anywhere
// above would renumber those `Location`s and move the loadable image even with the knob off — the
// PI-V3D-1 bisect-proven constraint the `smpmark`/ORIN-WM1/JD1-DC wire-ins all obey.

/// DESKSEAM — one-shot latch. `tegra_early_stop` runs once per boot on the boot core, so this cannot
/// fire today; it is here because the two `#[cfg]` mazes above this line have both grown a second
/// reachable path in the past, and `desktop_firmware::activate` has its own latch for exactly that reason.
#[cfg(all(target_arch = "aarch64", feature = "tegradesk"))]
static TEGRADESK_ENTERED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// DESKSEAM STOP-LINE 1 — **has rung 3 (input routing) landed?** `display_tegra::JD1_DC_PROBE`'s
/// idiom: a compile-time precondition held `false` in source, flipped by the rung that discharges it.
///
/// §6.1 of the ladder, which calls this ordering "not negotiable": `desktop_firmware::activate` opens the
/// console window, that window carries a minimise disc, and `video/desktop_firmware.rs:39-44` states the
/// CONSWIN law — the only route back from that park is the dock, and the dock is only a way back
/// once clicks route. **DISCHARGED 2026-08-25 by ORIN-CLICK (rung 3, orin-desktop.md §3.7):** that
/// arm now calls `wc_click_route` (`main.rs:2852`), behind `orinclick`. ⚠ NOT a literal `true`, and
/// the difference IS the safety property: `tegradesk` does NOT imply `orinclick`, so a hard `true`
/// would assert a route back on an image that has none — the one-way trip re-entered through the very
/// constant meant to prevent it. DERIVED FROM THE BUILD. Routing is UNFLOWN; §5.2 still holds.
#[cfg(all(target_arch = "aarch64", feature = "tegradesk"))]
const TEGRADESK_CLICK_ROUTED: bool = cfg!(feature = "orinclick");

/// DESKSEAM STOP-LINE 2 — **§5.2, the inherited stack hazard.** The Pi overflowed a 16 KiB kernel
/// stack in the desktop-arming cascade TWICE on consecutive metal boots (2026-08-22, boots 10 and
/// 11), and both commits state that no gate in this tree can reproduce it: the overflow needs a
/// preemption frame on an already-deep pass, and raspi4b delivers no timer IRQ on that path. A green
/// `test-arm` is therefore not evidence about this, and treating it as evidence is the failure.
///
/// Arming the full cascade is RUNG 5 and is blocked. Rung 2 lands the seam, not the desktop.
///
/// Flipping this alone is not enough and is not the intended route: the flip belongs to the rung that
/// can show the Orin's own `[u7stk]`/`[redzone]` numbers for this cascade on this board. §7 of the
/// ladder is explicit that every stack number quoted for it so far is a **Pi** number.
///
/// ⚠ **AND THAT CLEARING CONDITION CANNOT BE MET WHERE THE CASCADE RUNS** (ORIN-DESKFURN, 2026-08-26,
/// orin-desktop.md §3.12). `sched::stk_probe` loads `SCHED[cpu].current` and RETURNS EARLY when it is
/// null — which is exactly the boot core at this terminus, before `run_capstone_boot_core` drives the
/// queue. So every rung on this line is a place where `[u7stk]` is a no-op. Clearing §5.2 needs an
/// instrument this ladder does not have (a boot-stack high-water probe, or the cascade moved off the
/// boot stack); it does NOT license flipping this constant on the argument that the numbers are
/// unobtainable. Recorded here so the next reader of this stop-line meets the gap, not just the hold.
#[cfg(all(target_arch = "aarch64", feature = "tegradesk"))]
const TEGRADESK_CASCADE_OK: bool = false;

/// **DESKSEAM — the Orin's desktop seam: evaluate the floors, print them, and refuse.**
///
/// Returns `true` iff `desktop_firmware::activate()` both ran and left the console ROUTED. Every other path
/// returns `false` and names its reason on the wire. Nothing the caller does depends on the value
/// today — the tegra path has no `fbcon::detach()` handoff to guard, which is the one thing the Pi
/// wrapper's return value exists for — so this is a leaf, and a refusal costs the boot nothing.
///
/// **EVERY VERDICT BELOW IS DERIVED FROM A VALUE READ ON THIS BOOT, never asserted.** The floors are
/// the same ones `video/desktop_firmware.rs` evaluates internally, hoisted to the seam so that a refusal names
/// itself BEFORE the cascade is entered rather than from inside it — and two of them (`table` and
/// the stage) are floors `desktop_firmware` does not check at all, because on the Pi they cannot fail.
#[cfg(all(target_arch = "aarch64", feature = "tegradesk"))]
fn tegra_desk_arm() -> bool {
    use core::sync::atomic::Ordering;
    use unaos_kernel::video::{dock, fbcon, desktop_firmware, wm};

    if TEGRADESK_ENTERED.swap(true, Ordering::AcqRel) {
        serial_println!("[deskseam] REFUSE reason=already-armed (the seam is one-shot; a second pass would re-enter pidesk's own latch and re-ask for a window it would hand straight back)");
        return false;
    }

    // FLOOR 1 — THE PANEL, live off the surface the compositor composites onto, never assumed.
    // Copy the info out and drop `WRITER` immediately: nothing below may hold it across a `wm` call
    // (the WRITER/TABLE acquisition order, ORIN-WM1's rule). FAILS on a headless Orin — no DTB
    // `simple-framebuffer` handoff means the JD1 block printed `JB1b — geometry unresolved` and
    // never seeded `WRITER`, and every floor after this one is a function of panel geometry.
    let info = {
        let fb = *unaos_kernel::video::WRITER.lock();
        if !fb.is_ready() {
            serial_println!("[deskseam] REFUSE reason=no-panel (headless boot — JD1 seeded no scanout; every floor below is a function of panel geometry)");
            return false;
        }
        fb.info()
    };
    // NO SEPARATE ZERO-GEOMETRY FLOOR, and it was removed rather than never written: `desktop_firmware`'s own
    // step 1 tests `pw == 0 || ph == 0` after reading `WRITER`, so the obvious thing was to hoist
    // that test here too. `FrameBuffer::is_ready` (framebuffer.rs) is
    // `base != 0 && len != 0 && info.width != 0 && info.height != 0` — the extent test is already
    // inside the readiness test, so the hoisted arm was unreachable. It was written, and the ARMED
    // artifact convicted it: `LC_ALL=C grep -a -o` found every other `[deskseam]` string in
    // `unaos-kernel` and not that one, because the optimiser had proved it dead. A DECLINE arm that
    // cannot be reached is not a floor, it is a comment that costs image bytes.
    let (pw, ph) = (info.width, info.height);

    // FLOOR 2 — THE STAGING BUFFER (§3.3), and this is the floor the Orin uniquely needs. `wm`'s
    // staged presents run inside `SYS_WIN_PRESENT`'s IRQ mask, so a buffer that GREW on the pass
    // would be a masked acquisition of the global heap `Mutex` — the F1-F5 defect family. The tree's
    // only `reserve_stage` caller is `video::init_panel`, and the tegra path never reaches it (it
    // seeds `WRITER` by hand instead), so without this line every composite the cascade drives would
    // take the lazy-growth path. Grow-only and idempotent, so it is safe beside ORIN-WM1's own call.
    // FAILS when the 48 MiB aarch64 heap cannot spare entry 0 at all.
    let staged = wm::reserve_stage(&info);
    if staged == 0 {
        serial_println!("[deskseam] REFUSE reason=stage-unreserved panel={}x{} stage=0 (wm::reserve_stage got nothing for entry 0 — every composite would grow its buffer under SYS_WIN_PRESENT's IRQ mask, the F1-F5 shape)", pw, ph);
        return false;
    }
    let panel_bytes = pw.saturating_mul(ph).saturating_mul(info.bytes_per_pixel);
    let cover = if panel_bytes == 0 { 0 } else { staged.saturating_mul(100) / panel_bytes };

    // FLOOR 3 — THE WINDOW TABLE MUST BE EMPTY, read from `wm::count()`.
    // `video/desktop_firmware.rs` step 1b writes the WHOLE PANEL to `DESKTOP_BG` through the FRONT buffer, and
    // its own argument for why that direct write is sound is "the window table is empty at this
    // line, so a direct front-buffer write collides with no compositor-owned pixel". On the Pi that
    // is true by construction. On the Orin it is NOT: rung 0 (`orindesk`) mints and composites a row
    // ~400 lines above this one, so `UNAOS_ORINDESK=1 UNAOS_TEGRADESK=1` reaches this seam with a
    // live window on the glass and `desktop_firmware`'s clear would paint over it while asserting it could not.
    // This is a REACHABLE failing verdict from two knobs in `arroyo` today, not a hypothetical.
    let live = wm::count();

    // FLOOR 4 — THE DOCK, i.e. the CONSOLEWIN law's geometry half, evaluated with the SAME call
    // `desktop_firmware` makes (`MAX_WINDOWS`, not the live count — the check must hold for every table state
    // the boot can reach). `for_panel` is pure integer geometry and paints nothing. Two-sided in
    // codegen and derived from the live panel — but the WITHHELD arm's REACH is stated as measured
    // rather than inherited from the Pi: on this build `for_panel` INLINES and const-folds its
    // step-down loop to two compares, `ph >= 76 && pw >= 576` (`cmp x21, #0x4c` /
    // `ccmp x20, #0x240, hs` / `cset w20, lo` in the armed `unaos-kernel`), so no Orin panel this
    // branch has seen withholds it. The Pi's `DECLINE reason=dock-cannot-host-full-strip
    // panel=640x480 rows=12` was measured under that track's cell metrics and does NOT transfer —
    // 640x480 clears the floor above. Reported as the console-window CENSUS term rather than as a
    // refusal because a withheld console window is not fatal to `desktop_firmware` either: the bar follows
    // regardless, which is the one place `desktop_firmware`'s sequence deliberately differs from `desktop_uefi`'s.
    let cwin_ok = dock::Layout::for_panel(wm::MAX_WINDOWS, pw, ph).is_some();
    let routed_now = fbcon::console_is_routed();

    // THE CENSUS. Printed unconditionally, before any refusal below it, so a capture always carries
    // every floor's measured value and not merely the first one that said no.
    serial_println!(
        "[deskseam] floors panel={}x{}x{} stage={} cover={}% table={} console-window={} route={} click-route={} cascade={} rows={}",
        pw, ph, info.bytes_per_pixel,
        staged, cover, live,
        if cwin_ok { "GRANTED" } else { "WITHHELD" },
        if routed_now { "ROUTED" } else { "UNROUTED" },
        if TEGRADESK_CLICK_ROUTED { "LANDED" } else { "UNLANDED" },
        if TEGRADESK_CASCADE_OK { "CLEARED" } else { "HELD" },
        wm::MAX_WINDOWS
    );

    if live != 0 {
        serial_println!("[deskseam] REFUSE reason=table-not-empty live={} (pidesk's DESKTOP-CLEAR writes the whole panel through the FRONT buffer and its soundness argument is an EMPTY window table — ORIN-WM1's row is already composited on this glass; drop UNAOS_ORINDESK to arm the seam)", live);
        return false;
    }

    // THE STOP-LINES — ONE decision and ONE line, not two sequential gates, and the reason is a
    // MATCH over both terms rather than the first one that said no.
    //
    // That shape is not a style choice; the artifact forced it. Written as two sequential
    // `if !CONST { … return }` blocks, the second one's string is DEAD CODE the moment the first
    // const is `false` — `LC_ALL=C grep -a -o` on the armed `unaos-kernel` found `rung3-unlanded`
    // and did NOT find `stop-line-5.2`, so an operator's capture would have named the ordering
    // obligation and been silent about the STACK HAZARD, which is the more dangerous of the two.
    // The `match` keeps both in the string the build can actually print.
    if !(TEGRADESK_CLICK_ROUTED && TEGRADESK_CASCADE_OK) {
        let held = match (TEGRADESK_CLICK_ROUTED, TEGRADESK_CASCADE_OK) {
            (false, false) => "rung3-unlanded+stop-line-5.2",
            (false, true) => "rung3-unlanded",
            _ => "stop-line-5.2",
        };
        serial_println!("[deskseam] REFUSE reason={} console-window={} panel={}x{} stage={} (§6.1: pidesk::activate opens the console window, whose minimise disc's only route back is the dock, and jd2_console_pump's Event::Button arm still only logs the mask — the disc would be a one-way trip until rung 3 lands. §5.2: arming the full cascade is rung 5 and is BLOCKED — the Pi overflowed a 16 KiB kernel stack in it on two consecutive metal boots and no QEMU gate in this tree can stack the preemption frame that does it, so a green test-arm is not evidence here)", held, if cwin_ok { "GRANTED" } else { "WITHHELD" }, pw, ph, staged);
        return false;
    }

    // THE ARMED PATH. Type-checked by the `arm-tegra-seam` leg; not reached on any boot this branch
    // can produce, because both consts above are `false` in source. The verdict is DERIVED from the
    // call's own return CROSSED with `fbcon::console_is_routed()` read back afterwards — `activate`
    // already reads the route back rather than inferring it, and this line disagreeing with it would
    // itself be the finding.
    let activated = desktop_firmware::activate();
    let routed_after = fbcon::console_is_routed();
    let verdict = match (activated, routed_after) {
        (true, true) => "ROUTED",
        (false, false) => "UNROUTED",
        // `activate` returns `fbcon::console_is_routed()`; a disagreement means the route changed
        // between the two reads, which nothing on this path may do.
        _ => "INCOHERENT",
    };
    serial_println!(
        "[deskseam] ARMED panel={}x{} stage={} activate={} route={} -> {}",
        pw, ph, staged, activated, routed_after, verdict
    );
    activated && routed_after
}

// =================================================================================================
// ORIN-CONWIN (tail block) — rung 4's SECOND half: the handoff detach, guarded by the route.
// =================================================================================================
//
// THE TWO EDITS IN THIS FILE, and why neither adds a source line. Rung 4's entry is
// `arch::display_tegra::orin_conwin`, called as a statement APPENDED to `tegra_early_stop`'s terminus
// line beside DESKSEAM's — the `smpmark` / ORIN-WM1 / JD1-DC convention, whose reason is PI-V3D-1 and
// is bisect-proven rather than stylistic: `panic!`/`assert!`/bounds-check sites bake a
// `core::panic::Location` (file AND LINE) into rodata, so inserting even a comment line ahead of them
// shifts every later `Location` and the knob-off image stops being byte-identical. The second edit is
// `jd2_console_pump`'s phase-2 `fbcon::detach()`, folded IN PLACE to `if !tegra_conwin_live() { … }`.
// This block is at the FILE TAIL, below every `Location` in `main.rs`, so nothing moves.
//
// WHY THE GUARD IS THE WHOLE OF RUNG 4'S "LIVE" CLAIM. `detach` sets `GUI_ACTIVE`, after which
// `fbcon::_print` returns at its first test and not one further kernel line reaches the console by any
// path. A console window opened at the terminus and then detached at phase 2 would hold the boot log
// and never change again — the frozen snapshot x86's desktop lane ships (`desktop_uefi.rs` calls its row "a
// FROZEN BOOT-LOG SNAPSHOT for the rest of the boot"). The Orin does not have to inherit that, for the
// Pi's reason, restated for this board: the REASON for the detach is discharged by the route itself.
// The detach exists so that exactly one core writes the PANEL once the JD2 `Screen` console owns it —
// and a routed console does not write the panel. `FbCon::draw_fb` hands back `win_fb`, cached kernel
// RAM no scan-out reads, and the pixels reach glass only through `wm`'s staged present, damage-limited
// and paced. So on a boot where the route was installed, the handoff SKIPS the detach and the window
// stays LIVE; on every other boot the guard is `false` and phase 2 detaches exactly as it always did.
//
// FAIL-CLOSED BY CONSTRUCTION. `console_is_routed()` answers `false` for every decline arm the open
// path has AND for every arm `orin_conwin` itself takes — no panel, the ordering rule, the dock law,
// an open that declined — so a rung that refused anywhere leaves the detach taken, unchanged.

/// ORIN-CONWIN — **is the console routed into a compositor window on THIS boot?** The one fact
/// `jd2_console_pump`'s phase-2 detach is guarded by. Read from `fbcon` rather than from a local, for
/// `desktop_firmware::activate`'s reason: a route declined deep inside the open path must not be reported as
/// installed by a caller that remembers having asked for one.
///
/// ⚠ THE `target_arch` TERM IS FORCED AND ITS REASON IS MEASURED — it is not an arch reflex.
/// `arroyo`'s env map appends `tegra` to ONE shared feature list that BOTH legs of `./arroyo check`
/// are built with, so `feature = "tegra"` alone is TRUE on an x86_64 build; the callee
/// `fbcon::console_is_routed` is gated `all(target_arch = "aarch64", feature = "desktop_firmware")` and does not
/// exist there. That is an E0425 this arc's first check actually produced, not a hypothetical. Every
/// tegra helper at this file's tail carries the same term for the same reason (`jd2_console_pump`,
/// `tegra_rast_demo_maybe`, `tegra_desk_arm`, `tegra_el0_start_maybe`). The KNOB still does the
/// gating; the arch term only stops a `tegra`-flavoured x86 leg from compiling an aarch64 call.
#[cfg(all(feature = "tegra", feature = "orinconwin", target_arch = "aarch64"))]
#[inline(always)]
fn tegra_conwin_live() -> bool {
    unaos_kernel::video::fbcon::console_is_routed()
}

/// Knob-off (every tegra build without `orinconwin`, and every x86 leg `arroyo` hands `tegra` to): the
/// guard compiles to nothing. `#[inline(always)]` on a body that is a constant `false` means the call
/// site emits zero instructions and folds to the bare `fbcon::detach()` it has always been, so the
/// jetson image stays byte-identical.
#[cfg(all(feature = "tegra", not(all(feature = "orinconwin", target_arch = "aarch64"))))]
#[inline(always)]
fn tegra_conwin_live() -> bool {
    false
}

// =================================================================================================
// ORIN-SUPSTATE — SMP-redesign Candidate A arc 1 (the console-surface STATE LIFT). `supstate`,
// DEFAULT OFF. Appended at the file tail on purpose: knob-off these items are `#[cfg]`-erased and
// nothing below them exists to be shifted, so the jetson image's panic-`Location` records — and with
// them the knob-off byte-identity proof — are untouched (the orinclick line-neutrality rule).
// =================================================================================================

/// ORIN-SUPSTATE — `jd2_console_pump`'s phase 2 with the console surface LIFTED out of task-local
/// storage. Entered from the pump's phase-1 boundary (the `#[cfg]`-appended hook on the phase-1
/// loop's closing line) and never returns, exactly as the legacy phase 2 never returns.
///
/// WHAT IS DIFFERENT, and it is ownership only: `Screen` and `Console` are built here exactly as
/// the legacy phase 2 builds them — same construction order, same serial lines, same first-key
/// handling — and then INSTALLED into `display_tegra`'s module-owned handle (`sup_install`) instead
/// of living out the boot as this task's stack locals. Every subsequent surface access goes through
/// `sup_with_surface`, so the state outlives its holder: a successor task could adopt the very
/// `Screen`/`Console` this task was driving when it died. `TargetPal` is a VIEW (`&mut Screen`) and
/// is reconstructed per lock scope via its public `surface` field — `TargetPal::new` is called
/// exactly once, below, so the armed boot carries the same single `:: UI1:` line the baseline does.
///
/// WHAT IS DELIBERATELY THE SAME: the drive loop below is the legacy pump's phase-2 loop, clause
/// for clause (xHCI poll, coalesced pointer drain, save-under cursor bracket, ~250 ms sweep cadence
/// with the census on the same counter phase 1 advanced, `yield_now` — never `sleep_ticks`, the JC3
/// rule: this core's terminus drains no sleepers). Behavioural falsifier: the `[orinclick]` census
/// and the `:: tegra: JD2 —` transcript, which must read identically knob-on vs knob-off.
///
/// Milestone 1 of the arc: ONE task still holds every role. The role split (input source /
/// presenter / dispatcher, separate task identities, all still pinned to core 0) lands on top of
/// this lift as the arc's second milestone; the supervisor/reap/restart half is arc 2's, gated on
/// Candidate B's clock, and nothing here pretends otherwise.
#[cfg(all(feature = "tegra", target_arch = "aarch64", feature = "supstate"))]
fn jd2_supstate_phase2(
    front_fb: unaos_kernel::video::FrameBuffer,
    first_key: Option<u8>,
    sweep_ticks: u64,
    mut last_sweep: u64,
    mut sweep_tick: u64,
) {
    use unaos_kernel::arch::display_tegra::sup_install;
    use unaos_kernel::pal::{Event, GneissPal};

    // The pump's free-running clock, re-read here rather than passed: CNTPCT is EL-independent and
    // per-core monotonic (the JD3 timerless mechanism), and `last_sweep`/`sweep_tick` carry the
    // phase-1 cadence across the boundary so the sweep counter never restarts.
    let cntpct = || -> u64 {
        let v: u64;
        unsafe {
            core::arch::asm!("mrs {}, CNTPCT_EL0", out(reg) v, options(nomem, nostack, preserves_flags));
        }
        v
    };

    // ——— the JD2/JD4 panel takeover: legacy phase 2, verbatim semantics ———————————————————————————
    // Detach the fbcon serial mirror FIRST (guarded by the ORIN-CONWIN route exactly as the legacy
    // line is), so a CAPSTONE straggler can't paint over the console frame.
    if !tegra_conwin_live() {
        unaos_kernel::video::fbcon::detach();
    } #[cfg(feature = "rast")] unaos_kernel::arch::display_tegra::orin_rast_console_owns(); // ORIN-RASTGLASS: the phase-2 boundary latch, MISSING FROM THIS COPY until this arc and that omission produced a WRONG VERDICT ON A HEALTHY BOOT. `jd2_supstate_phase2` never returns (the drive loop at this function's tail is non-breaking), so on a `supstate` image the legacy phase 2 — the only site that latched `RG_CONSOLE_OWNS` — is unreachable and the latch was never set. Read off `display_tegra::orin_rast_census`: with `owns` false, a console that has taken the panel and repainted the cube away scores `RAST-PAINTED-OVERWRITTEN`, which that census calls "the only arm that indicts a repainter", instead of `RAST-SUPERSEDED-BY-CONSOLE`. A correct boot was being reported as a defect. Placed exactly as the legacy twin places it: on the FIRST statement of phase 2 and OUTSIDE the conwin guard, because the console takes the panel on both routes and only the detach differs. ⚠ LINE-NEUTRAL append.
    let mut screen = unaos_kernel::video::Screen::new(front_fb);
    // VUGRAS: the Screen back buffer PA is only known now — add it to the candidate table.
    unaos_kernel::vugras::note_screen(&screen);
    // The single `TargetPal::new` of the armed boot — prints the one `:: UI1:` line.
    let mut pal = unaos_kernel::pal::TargetPal::new(&mut screen);
    let mut console = unaos_kernel::console::Console::new();
    // JD11: mirror every command-output line to serial (durable, mbench-able transcript).
    console.set_output_sink(jd2_out_sink);
    console.println("UnaOS — Jetson Orin Nano (Tegra234)");
    console.println("JD2: interactive shell on the inherited scanout. Type 'help'.");
    console.draw(&mut pal);
    pal.render();
    match first_key {
        Some(c) => {
            serial_println!( // ORINPATH — `path=` DISCRIMINATES THIS COPY FROM THE LEGACY ONE, and until this arc it did not: `jd2_console_pump`'s two takeover literals and these two were BYTE-IDENTICAL, so neither a serial capture nor a grep of the artifact could say which site a boot had printed — the one question the state-lift's own falsifier turns on. ⚠ MEASURED, not argued (arm-tegra-supstate kernel.elf at 0ed6fee2): `LC_ALL=C grep -a -o` found exactly ONE `.rodata` run for each of the two literals, not two — both copies pass `#[cfg]`, but `jd2_supstate_phase2` never returns, so LLVM drops the legacy phase 2 as unreachable. That is WORSE than the two-run case it looked like, because a single unattributable run reads as confirmation: the one surviving copy was the one this function prints, and nothing in the image said so. With the token in place the same grep answers it — `path=jd2-supstate-phase2` present, `path=jd2-console-pump` absent. The token is deliberately longer than 8 bytes: a witness mark of 8 or fewer can be LLVM-immediate-encoded and never reach `.rodata` at all, which would defeat the artifact-grep law this tree runs on. Placed AFTER "console OWNS the panel" on purpose: `scripts/specs/jetson-sync1.spec` and `jetson-jd5.spec` both `REQUIRE JD4.*console OWNS the panel`, and that regex still matches. ⚠ This deliberately BREAKS the "must read identically knob-on vs knob-off" property this function's doc comment names as its behavioural falsifier — being unable to tell the two transcripts apart was never evidence that they were equivalent, it was the absence of evidence either way.
                ":: tegra: JD2 — console OWNS the panel (Screen back buffer live); path=jd2-supstate-phase2; first key {:#04x} ::",
                c
            );
            // The wake-up keystroke is a real keystroke: feed it through, don't swallow it.
            handle_key(c, &mut console, &mut pal);
            pal.render();
        }
        None => serial_println!(
            ":: tegra: JD4 — console OWNS the panel (Screen back buffer live); path=jd2-supstate-phase2; screen-on-boot (no key, ~8 s) ::"
        ),
    }

    // ——— THE LIFT: the surface leaves this task's stack and becomes module-owned ————————————————
    drop(pal); // end the view's borrow; the Screen it viewed moves into the handle next
    sup_install(screen, console);

    // ——— THE SPLIT (milestone 2): the roles get separate task identities ————————————————————————
    // Both new roles are PINNED TO CORE 0 exactly as this task is — this arc changes ownership,
    // never placement. Spawned only now, after `sup_install`, so neither role can ever observe an
    // uninstalled surface. This task carries on as the INPUT SOURCE below.
    unaos_kernel::arch::sched::spawn("jd2-present", jd2_supstate_presenter, 0, 0);
    unaos_kernel::arch::sched::spawn("jd2-dispatch", jd2_supstate_dispatcher, 0, 0);
    serial_println!(
        "[supstate] roles input=jd2-console presenter=jd2-present dispatcher=jd2-dispatch core=0 (pinned; ownership split, placement unchanged) -> SPLIT"
    );

    // ——— the INPUT SOURCE loop: xHCI poll + PAL drain + click routing + the sweep cadence ————————
    // What stays here is exactly the legacy loop's input half: the controller poll, the event
    // drain with per-frame pointer coalescing, the JD20 announce/Button lines, `orin_click`, and
    // the ~250 ms sweep (VUGRAS + the `[orinclick]` census — the census stays AUTHORED BY THE
    // ROUTING TASK, which is the property that makes it trustworthy; see display_tegra.rs).
    // What left: keys go over the key seam to the dispatcher; presentation goes to the board.
    let mut announced = false;
    loop {
        if let Ok(mut x) = unaos_kernel::drivers::xhci::claim() {
            x.poll_events();
        } #[cfg(feature = "orinrx")] unaos_kernel::arch::serial::serialrx::drain(); // SERIALRX (ORINRX) — the `supstate` twin of `jd2_console_pump`'s drain: this function never returns, so on a `supstate` image the legacy phase 2 is dead code and WITHOUT this site the knob would be inert there (the ORIN-RASTGLASS omission shape). ⚠ LINE-NEUTRAL append.
        // ORIN-POINTER-FIX: coalesce all pointer motion in the frame to a single cursor update —
        // per-frame cursor work is O(1) regardless of report rate (see the legacy loop's account).
        let mut pending_rel: Option<(i32, i32)> = None;
        let mut pending_abs: Option<(i32, i32)> = None;
        // GUI-CLICK-2's contract, honoured across the split: while a foreground command owns the
        // screen, its own pump_and_poll is the PAL queue's consumer — this drain stands down and
        // only the xHCI poll above keeps feeding the ring (claim() is try-based on both sides).
        // And the key seam's bound is checked BEFORE each pop, so a key is never popped-then-lost:
        // when the dispatcher is 64 keys behind, events wait in the PAL ring exactly as they do
        // today while the monolithic pump is busy inside `handle_key`.
        if !SCREEN_APP_ACTIVE.load(core::sync::atomic::Ordering::Relaxed) {
            while !unaos_kernel::arch::display_tegra::sup_key_full() {
                let Some(ev) = unaos_kernel::pal::next_event() else { break };
                match ev {
                    Event::Key(c) => {
                        // Cannot fail: the seam's bound was checked before the pop above.
                        let _ = unaos_kernel::arch::display_tegra::sup_key_push(c);
                    }
                    Event::Mouse { x, y } => {
                        let (ax, ay) = pending_rel.unwrap_or((0, 0));
                        pending_rel = Some((ax + x, ay + y));
                        if !announced {
                            serial_println!(
                                ":: tegra: JD20 — pointer live (relative mouse, cursor on scanout) ::"
                            );
                            announced = true;
                        }
                    }
                    Event::MouseAbsolute { x, y } => {
                        pending_abs = Some((x, y));
                        if !announced {
                            serial_println!(
                                ":: tegra: JD20 — pointer live (absolute tablet, cursor on scanout) ::"
                            );
                            announced = true;
                        }
                    }
                    Event::Button(mask) => {
                        // JD20's raw line first (an event reached the PUMP), then the ORIN-CLICK
                        // route — the same two separable claims the legacy arm keeps separable.
                        serial_println!(":: tegra: JD20 — pointer BUTTON {:#04x} (down) ::", mask);
                        #[cfg(feature = "orinclick")]
                        unaos_kernel::arch::display_tegra::orin_click(mask);
                    }
                    _ => {}
                }
            }
        }
        if pending_rel.is_some() || pending_abs.is_some() {
            unaos_kernel::arch::display_tegra::sup_frame_pointer(pending_rel, pending_abs);
        }
        // VUGRAS writeback sweep + the ORIN-CLICK census, on the cadence phase 1 advanced.
        if cntpct().wrapping_sub(last_sweep) >= sweep_ticks {
            last_sweep = cntpct();
            unaos_kernel::vugras::idle_sweep(sweep_tick); unaos_kernel::arch::display_tegra::sup_present_census(sweep_tick); #[cfg(feature = "rast")] unaos_kernel::arch::display_tegra::orin_rast_census(sweep_tick); #[cfg(feature = "orinrx")] unaos_kernel::arch::serial::serialrx::census(sweep_tick); // SERIALRX (ORINRX) — the `[serialrx] rx=` census on the `supstate` sweep. ⚠ LINE-NEUTRAL append. // ORIN-SUPSOUND — the ~10 s presenter census, authored by the INPUT SOURCE on purpose: this board's UART is shared with the SPE's TCU and drops bytes, so a dead presenter must produce a REPEATING LINE (`pass=+0 -> DEAD`), never a silence. Appended to an existing statement, and un-`#[cfg]`ed because this whole block is already `supstate`-gated. ORIN-RASTGLASS: the rast census is the phase-2 twin of the sweep-site call in `jd2_console_pump`'s phase-1 loop and it was MISSING from this copy, so on a `supstate` image the census stopped dead at the phase-1 boundary and the rung's verdict was whatever the last phase-1 sweep had said — with `RG_CONSOLE_OWNS` never latched (see the phase-2 boundary line above), permanently pre-takeover. The census self-terminates on `RG_DONE`, so restoring it here costs one relaxed load per sweep once the question is answered. ⚠ LINE-NEUTRAL append.
            #[cfg(feature = "orinclick")]
            unaos_kernel::arch::display_tegra::orin_click_census(sweep_tick);
            sweep_tick += 1;
        }
        unaos_kernel::arch::sched::yield_now();
    }
}

/// ORIN-SUPSTATE — the DISPATCHER role (`jd2-dispatch`, pinned core 0): pop keys off the seam,
/// echo them (the bench evidence line — panel + serial must agree, and the echo lives with the
/// task that makes the panel agree), and run `handle_key` -> `shell::dispatch_command` under the
/// surface lock. Sequential by construction, so the per-key ordering echo -> command output that
/// the `awk '/:: tegra: JD2 —/'` transcript reconstruction relies on is preserved exactly.
///
/// The one deliberate lock-discipline exception lives here: this task HOLDS the surface across
/// `handle_key`, whose full-screen commands (`vug`, `gneiss`) yield from their own drain loops.
/// Safe under the module's rule — every other waiter is `try_lock` + `yield_now`, so the holder's
/// command keeps running — and identical to today's semantics, where the monolithic pump was
/// blocked inside `handle_key` and presented nothing until the command returned.
#[cfg(all(feature = "tegra", target_arch = "aarch64", feature = "supstate"))]
fn jd2_supstate_dispatcher(_arg: usize) {
    use unaos_kernel::arch::display_tegra::{sup_frame_key_repaint, sup_key_pop, sup_with_surface};
    use unaos_kernel::pal::cursor;
    loop {
        let mut handled = false;
        while let Some(c) = sup_key_pop() {
            handled = true;
            let took = sup_with_surface(|screen, console| {
                let mut pal = unaos_kernel::pal::TargetPal { surface: screen };
                // Erase the cursor BEFORE the console repaints, so the save-under stash never
                // captures cursor pixels (the legacy bracket, unchanged).
                cursor::restore(&mut pal);
                if (32..=126).contains(&c) {
                    serial_println!(":: tegra: JD2 — KEY '{}' ::", c as char);
                } else {
                    serial_println!(":: tegra: JD2 — KEY {:#04x} ::", c);
                }
                handle_key(c, console, &mut pal)
            })
            .unwrap_or(false);
            if took {
                // A command took the whole screen: stop draining this pass, exactly as the legacy
                // frame stopped, so a queued keystroke can't paint the console back over it.
                break;
            }
        }
        if handled {
            // The console repainted into the back buffer; the presenter re-composites the cursor
            // on top and presents (the legacy frame's key_repainted/needs_render pair, as a mark).
            sup_frame_key_repaint();
        }
        unaos_kernel::arch::display_tegra::sup_dispatch_pass(); unaos_kernel::arch::sched::yield_now(); // ORIN-SUPSOUND — the dispatcher's own liveness numerator: `disp=+0` beside a live `pass=+N` says the keys stopped being drained while the panel kept presenting, which is a different defect from a dead presenter and reads identically on the wire without this.
    }
}

/// ORIN-SUPSTATE — the PRESENTER role (`jd2-present`, pinned core 0): drain the frame board and
/// own every flush to glass — the coalesced cursor composite (R22 save-under bracket), the
/// re-composite over a key repaint, the ~1.5 s auto-hide erase, and `pal.render()`. Nothing else
/// presents: the dispatcher's `handle_key` draws only into the back buffer (the `pal.render`
/// calls inside full-screen commands own the panel by definition and are unchanged).
#[cfg(all(feature = "tegra", target_arch = "aarch64", feature = "supstate"))]
fn jd2_supstate_presenter(_arg: usize) {
    use unaos_kernel::arch::display_tegra::{sup_frame_take, sup_with_surface};
    use unaos_kernel::pal::{cursor, GneissPal};
    let mut last_visible = cursor::visible();
    // ORIN-SUPSOUND — the presenter's FIRST wire line, and the only one it prints itself: it means
    // "this task was DISPATCHED", which `spawn`'s own `:: SCHED: task` witness cannot say on this
    // board (that line is `#[cfg(feature = "pi")]`, a deliberate wire-cost re-gate). Everything
    // after this is carried by the ~10 s `[suppresent] census`, which the INPUT SOURCE prints —
    // see the ORIN-SUPSOUND header in display_tegra.rs for why a lossy UART forbids a witness
    // whose death signal is silence.
    unaos_kernel::arch::display_tegra::sup_present_up();
    loop {
        let board = sup_frame_take();
        // The auto-hide transition is edge-detected across passes, exactly as the legacy frame
        // edge-detected it across one drain (`cursor_was_visible` at frame start vs after).
        let vis_now = cursor::visible();
        let auto_hide_erase = last_visible && !vis_now;
        last_visible = vis_now;
        // ORIN-SUPSOUND — `work` (the board had something to present) and `flushed` (pixels
        // actually reached glass) are separated deliberately: work-without-flush is the STALLED
        // verdict, the frozen-glass failure that a happy-path-only witness cannot distinguish
        // from health. The closure now RETURNS `needs_render` so the flush is observed rather
        // than assumed; `unwrap_or(false)` folds the no-surface case into "did not present".
        let work = board.rel.is_some() || board.abs.is_some() || board.key_repaint || auto_hide_erase;
        let mut flushed = false;
        if work {
            flushed = sup_with_surface(|screen, _console| {
                let mut pal = unaos_kernel::pal::TargetPal { surface: screen };
                let mut needs_render = board.key_repaint;
                if board.rel.is_some() || board.abs.is_some() {
                    // One cursor composite for the board's whole pointer activity (legacy bracket).
                    cursor::restore(&mut pal);
                    if let Some((dx, dy)) = board.rel {
                        cursor::move_rel(dx, dy, pal.width() as i32, pal.height() as i32);
                    }
                    if let Some((ax, ay)) = board.abs {
                        cursor::set_abs(ax, ay, pal.width() as i32, pal.height() as i32);
                    }
                    cursor::draw_over(&mut pal);
                    needs_render = true;
                } else if board.key_repaint && cursor::visible() {
                    // A key repaint erased the cursor and redrew the console; put the arrow back.
                    cursor::draw_over(&mut pal);
                }
                if auto_hide_erase {
                    // Auto-hide swept the cursor off: erase it once so no arrow is left parked.
                    cursor::restore(&mut pal);
                    needs_render = true;
                }
                if needs_render {
                    pal.render();
                }
                needs_render
            })
            .unwrap_or(false);
        }
        unaos_kernel::arch::display_tegra::sup_present_pass(work, flushed); unaos_kernel::arch::sched::yield_now(); // ORIN-SUPSOUND — two relaxed atomic adds on the per-frame path, no lock and no print; the ~10 s census turns them into the liveness verdict.
    }
}

// =================================================================================================
// ORIN-DESKFURN — THE FURNITURE. `orinfurn`, DEFAULT OFF. The Orin's menu bar is ENABLED and PAINTED
// and the SHARD menu becomes pressable. Ladder: docs/dev/OS/08_VIDEO/orin-desktop.md §3.12.
// =================================================================================================
//
// THE DEFECT THIS CLOSES. `menubar::ENABLED` starts `false` and there are exactly TWO
// `menubar::set_enabled(true)` calls in this tree: `video/desktop_uefi.rs:552` (x86) and
// `video/desktop_firmware.rs:292`. Neither is reachable on this board — `desktop_uefi` is x86_64-only, and `desktop_firmware`'s
// is inside `activate()`, which the tegra path never calls (see the DESKSEAM block above: that seam
// exists, IS wired to the terminus, and REFUSES at `TEGRADESK_CASCADE_OK`). So the Orin composites
// windows with no bar above them, and a press where the crystal should be lands on empty desktop.
// `strip`, `dock`, `menubar` and `crystal` are all COMPILED into any image carrying `desktop_firmware`, and
// `wc_click_route`'s furniture arm already calls `strip::press_route` -> `crystal::press_at` ahead of
// every window arm (display_tegra.rs's rung-4 block records exactly this). Everything is present;
// nothing turns it on. This turns it on.
//
// ⚠ WHAT THIS IS NOT, AND THE STOP-LINE IS NOT CROSSED. `desktop_firmware::activate()` is NOT called and
// `TEGRADESK_CASCADE_OK` is NOT touched — §5.2 stands and rung 5 is still blocked. `activate`'s
// sequence is nine steps; this takes TWO, `menubar::set_enabled(true)` and `wm::composite()`, because
// those two ARE the bar and nothing else in `activate` is. What is deliberately NOT taken, each for
// its own reason rather than as a blanket: the step-1b DESKTOP-CLEAR (a whole-panel write through the
// FRONT buffer whose soundness argument is an EMPTY window table — false here the moment rung 0 or
// rung 4 has minted a row); the console window (rung 4's, `orinconwin`, and it stands alone);
// `crystal::routed_selftest` (a `witness` fixture that drives presses through the live router — the
// deep arm, and the one closest in shape to Pi boot 11's overflow); `pulsewin::open`; the window
// population; the tegra `render_service` (§3.6, unbuilt); and the closing filesystem walk.
//
// WHY THE STACK ARGUMENT HOLDS HERE WHERE §5.2's DOES NOT. §5.2's hazard is the CASCADE's DEPTH and
// its named overflow is `quarry::open()` at click-router depth (Pi boot 11). `orinfurn` does not
// imply `quarry`, so that call is the `not(feature = "quarry")` `false` stub in this build — the same
// fact rung 4 leaned on. What this adds to the terminus is ONE more `wm::composite()`: the same call
// ORIN-WM1 (rung 0), `orin_conwin`'s `create_at` (rung 4) and `orin_tenant_arm` (rung 6) already make
// from this same line on the boot core's own entry frame, all three FLOWN on Orin metal (boot7f,
// boot7g, boot7h). A fourth instance of a shape this board has run three times is not a new shape.
//
// ⚠ AND THAT IS AN ARGUMENT FROM THE LEDGER, NOT A MEASUREMENT — said plainly because §7 of the
// ladder exists to stop exactly this sentence from being read as a number. No `[u7stk]` reading for
// this line exists on this board, and `sched::stk_probe` CANNOT take one here: it loads
// `SCHED[cpu].current` and returns early when that is null, which is precisely the boot core before
// `run_capstone_boot_core` drives the queue. So the terminus is a place where §5.2's own evidence
// requirement is structurally unsatisfiable. That is a finding ABOUT the stop-line, not a licence to
// step over it, and it is why this block narrows the work instead of flipping the constant.
//
// DEAD-STUB (the second edit in this file, and the whole of it is one `#[cfg]`). The knob-off
// `desktop_firmware_activate_maybe` above read `all(aarch64, not(all(desktop_firmware, baremetal)))`, which compiled the
// constant-`false` stub on every NON-baremetal aarch64 build — including every tegra one, where its
// only call site does not exist: that site is `kernel_main`'s GUI-handoff line, itself
// `all(aarch64, baremetal)`, and on this board `kernel_main` is unreachable anyway because
// `tegra_early_stop` is `-> !` and diverges. Hence `desktop_firmware_activate_maybe is never used` in every
// tegra gate log for weeks, and the warning was right. Narrowed to
// `all(aarch64, baremetal, not(desktop_firmware))` so the pair exists exactly where its caller does.
// BEHAVIOURALLY INERT: under `baremetal` both arms resolve exactly as before, so the Pi is
// unaffected; elsewhere it deletes a function that emitted no instructions. LINE-NEUTRAL: one
// attribute and its comment rewritten in place, no line added or removed in `main.rs`, so no panic
// `Location` moves and the knob-off byte-identity proof is intact.
//
// ZERO SOURCE LINES. The call site is a statement APPENDED to the existing terminus line and this
// block is at the FILE TAIL — the `smpmark`/ORIN-WM1/DESKSEAM/ORIN-CONWIN convention, whose reason is
// PI-V3D-1 and is bisect-proven: panic `Location` records embed line numbers, so a line inserted
// ahead of them moves the loadable image even with every knob off.
//
// WHERE ON THE LINE. LAST of the rungs — after `orin_ladder_arm`, before `tegra_rast_demo_maybe` —
// for two reasons. (1) Every earlier rung's probe (`[orinwm1]`, `[orinchrome]`, `[orinconwin]`,
// `[orintenant]`, `[oringlass]`, `[orindock]`) then still reads a panel with no bar on it, so an
// armed capture stays directly comparable to boot7f/7g/7h. (2) It is still AHEAD of
// `boot_ok_disarm`, so the `orinwdt` boot watchdog covers the composite this drives — if the bar's
// composite is where the board dies, the watchdog is still armed to say so.

/// DESKFURN — one-shot latch, for DESKSEAM's stated reason: `tegra_early_stop` runs once per boot on
/// the boot core so this cannot fire today, but the `#[cfg]` maze above has grown a second reachable
/// path before, and `menubar::set_enabled` is not idempotent in its WITNESS (it bumps `TOGGLES` and
/// latches `DEFAULT_LATCH`), so a second pass would make the bar's own instrument read a history that
/// did not happen.
#[cfg(all(target_arch = "aarch64", feature = "orinfurn"))]
static ORINFURN_ENTERED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// **ORIN-DESKFURN — enable the menu bar, composite it, and READ THE PAINT BACK.**
///
/// Returns `true` iff the bar is enabled AND owns pixels when this returns. Nothing the caller does
/// depends on the value: like DESKSEAM this is a leaf, because the tegra path has no
/// `fbcon::detach()` handoff for a return value to guard — rung 4's `tegra_conwin_live` guards that
/// one, off the ROUTE, and the bar has no bearing on it.
///
/// **EVERY VERDICT IS DERIVED FROM A VALUE READ ON THIS BOOT.** The floors are read live; the paint is
/// read back through `menubar::owns_pixels()` rather than inferred from having called `composite` —
/// `video/menubar.rs` documents that load as the one thing distinguishing "enabled" from "on the
/// glass", and `desktop_firmware` reads it for the same reason at the same seam.
#[cfg(all(target_arch = "aarch64", feature = "orinfurn"))]
fn tegra_desk_furn() -> bool {
    use core::sync::atomic::Ordering;
    use unaos_kernel::video::{menubar, wm};

    // THE ENTRY LINE, printed before anything can decline. A silent `false` must never be
    // indistinguishable from "the seam was never called" — that ambiguity is exactly what made the
    // missing bar read as a mystery rather than as a refusal, and one unconditional line closes it.
    // The sibling knobs are named because this rung's usefulness is a function of them: without
    // `orinconwin` the bar has no window caption to show, and `orinclick` is IMPLIED by `orinfurn`
    // (Cargo.toml) so `click=1` here is a structural fact, printed to be falsifiable rather than
    // trusted.
    serial_println!(
        "[orinfurn] arm click={} conwin={} desk={} tenant={} deskseam={} (ORIN-DESKFURN entered on the tegra terminus; §5.2 UNCROSSED — pidesk::activate is NOT called)",
        cfg!(feature = "orinclick") as u8,
        cfg!(feature = "orinconwin") as u8,
        cfg!(feature = "orindesk") as u8,
        cfg!(feature = "orintenant") as u8,
        cfg!(feature = "tegradesk") as u8
    );

    // ORIN-STKDEPTH — THE SECOND SP READ, beside the arm line and unconditional with it. Subtracted
    // from the anchor latched on `kernel_main`'s first statement, it is the exact number of bytes of
    // boot-core stack the chain `kernel_main -> tegra_early_stop -> tegra_desk_furn` has CONSUMED at
    // this instant. ⚠ DEPTH CONSUMED, NOT HEADROOM — the wire says so too, and the ORIN-STKDEPTH block
    // at this file's tail gives the full reason no headroom number is derivable on the Orin's UEFI
    // path (firmware stack, never switched; no `__stack_top` on this link; the bounding `MemoryRegion`
    // slice consumed by `memory::init`). §5.2 asks for BOTH; this clears the half that is takeable and
    // names the half that is not, which is the opposite of clearing the stop-line by argument.
    let stk_here: u64;
    unsafe {
        core::arch::asm!("mov {}, sp", out(reg) stk_here, options(nomem, nostack, preserves_flags));
    }
    let stk_anchor = ORINSTK_ANCHOR_SP.load(Ordering::Relaxed);
    if stk_anchor != 0 && stk_anchor >= stk_here {
        serial_println!(
            "[orinstkdepth] depth-consumed={} bytes anchor-sp={:#x} seam-sp={:#x} at=orinfurn-arm chain=kernel_main->tegra_early_stop->tegra_desk_furn -> DEPTH-CONSUMED (this is stack CONSUMED between the two reads and is NOT headroom: the Orin boot stack is UEFI's and is never switched, this link defines no __stack_top, and the MemoryRegion slice that would bound it is consumed by memory::init — so no remaining-headroom number is derivable in-kernel today and none is claimed here)",
            stk_anchor - stk_here, stk_anchor, stk_here
        );
    } else {
        serial_println!(
            "[orinstkdepth] DEPTH-UNAVAILABLE anchor-sp={:#x} seam-sp={:#x} at=orinfurn-arm reason={} (no number is printed rather than a number derived from an unset or non-monotonic anchor — an instrument that cannot fire in the state it exists for must say so, not guess)",
            stk_anchor, stk_here,
            if stk_anchor == 0 { "anchor-never-ran" } else { "anchor-below-seam (the two reads are not on one descending frame chain — the stack was switched between them)" }
        );
    }

    if ORINFURN_ENTERED.swap(true, Ordering::AcqRel) {
        serial_println!("[orinfurn] REFUSE reason=already-armed (the seam is one-shot; a second pass would re-toggle ENABLED, bump menubar's TOGGLES counter and make its DEFAULT_LATCH witness read a history that did not happen)");
        return false;
    }

    // FLOOR 1 — THE PANEL, off the surface the compositor composites onto, never assumed. Copy the
    // info out and drop `WRITER` immediately: nothing below may hold it across a `wm` call, which is
    // ORIN-WM1's WRITER/TABLE acquisition-order rule. FAILS on a headless Orin — no DTB
    // `simple-framebuffer` handoff means JD1 printed `JB1b — geometry unresolved` and seeded no
    // scanout, and every floor below is a function of panel geometry.
    let info = {
        let fb = *unaos_kernel::video::WRITER.lock();
        if !fb.is_ready() {
            serial_println!("[orinfurn] REFUSE reason=no-panel (headless boot — JD1 seeded no scanout; the bar's geometry, its floors and the composite that paints it are all functions of panel geometry)");
            return false;
        }
        fb.info()
    };
    let (pw, ph) = (info.width, info.height);

    // FLOOR 2 — THE STAGING BUFFER, here for DESKSEAM's reason restated: `wm`'s staged presents run
    // inside `SYS_WIN_PRESENT`'s IRQ mask, so a stage that GREW on the pass would be a masked
    // acquisition of the global heap `Mutex` — the F1-F5 defect family. The tree's only
    // `reserve_stage` caller on a normal boot is `video::init_panel`, which the tegra path never
    // reaches (it seeds `WRITER` by hand instead). Grow-only and idempotent, so it is safe beside the
    // identical calls rungs 0/2/4/6 make from this same line. FAILS when the 48 MiB aarch64 heap
    // cannot spare entry 0 at all.
    let staged = wm::reserve_stage(&info);
    if staged == 0 {
        serial_println!("[orinfurn] REFUSE reason=stage-unreserved panel={}x{} stage=0 (wm::reserve_stage got nothing for entry 0 — the bar's composite would grow its buffer under SYS_WIN_PRESENT's IRQ mask, the F1-F5 shape)", pw, ph);
        return false;
    }

    // FLOOR 3 — THE BAR'S OWN GEOMETRY FLOOR, read through THE ONE accessor (`menubar::strip_rect`,
    // which is also what `strip::TENANTS` and `wm::erase_clip` read — there is no second copy of the
    // bar's rectangle anywhere). `None` on a panel shorter than `FLOOR_H` or narrower than `FLOOR_W`.
    // REACHABLE rather than hypothetical: the floor admits 640x480 and refuses below it, so a boot
    // that came up on a fallback mode instead of the bench's 1920x1200 can land here.
    let rect = menubar::strip_rect(pw, ph);
    let live = wm::count();
    let was_enabled = menubar::enabled();
    let owned_before = menubar::owns_pixels();

    // THE CENSUS. Unconditional, and ahead of the refusal below it, so a capture always carries every
    // measured floor and not merely the first one that said no.
    serial_println!(
        "[orinfurn] floors panel={}x{}x{} stage={} rect={:?} table={} bar-was={} owned-was={} rows={}",
        pw, ph, info.bytes_per_pixel, staged, rect, live, was_enabled, owned_before, wm::MAX_WINDOWS
    );

    if rect.is_none() {
        serial_println!("[orinfurn] REFUSE reason=panel-below-bar-floor panel={}x{} (menubar::strip_rect declined this panel — shorter than FLOOR_H or narrower than FLOOR_W; enabling the bar anyway would subtract rows from every present that no composite could then fill)", pw, ph);
        return false;
    }

    // THE ARM — the tenancy is CLAIMED. Two statements, and the second is load-bearing rather than a
    // flourish: from the instant the bar is enabled `Screen::present_background` subtracts its rect,
    // so those rows stop being desktop pixels and ONLY a composite can fill them, and
    // `wm::service_damage` returns without compositing while no row is dirty. On the quiet boot this
    // board actually has, nothing would ever paint them. One pass at the seam closes that by
    // construction — `desktop_firmware`'s step 5, and SHELLDESK's exact symptom, reached on a third seam.
    let bar_was = menubar::set_enabled(true);
    wm::composite();

    // READ THE PAINT BACK; do not infer it from having called `composite`. `strip::paint` declines a
    // pass without touching a pixel on a contended `SCRATCH` (the dock is tenant #1 and takes the
    // same scratch in the same pass) or a surface not yet `word4`, and `menubar::compose` then
    // returns `false` with its slot untouched. One re-run discharges it — `owns_pixels` is the same
    // packed load `compose` acts on, so the retry runs iff the first pass did not land.
    let retried = if menubar::enabled() && !menubar::owns_pixels() {
        wm::composite();
        true
    } else {
        false
    };
    let painted = menubar::owns_pixels();

    if !painted {
        // ROLLBACK, and this is where this seam deliberately DIFFERS from `desktop_firmware::activate`, which
        // reports the miss and leaves the bar ENABLED. On the Orin that outcome is worse than no bar:
        // `jd2_console_pump` owns the panel through a `Screen` whose `present_background` subtracts
        // the bar's rect on every present, so an enabled-but-unpainted bar is a permanent dead band
        // across the top of the console for the rest of the boot, with no damage condition able to
        // notice it. Putting `ENABLED` back makes those rows desktop pixels again and the next
        // present fills them. The disagreement with `desktop_firmware` is stated rather than hidden: it is a
        // behaviour difference between two seams, and only a bench boot can say which is right here.
        menubar::set_enabled(bar_was);
        serial_println!(
            "[orinfurn] REFUSE reason=composite-declined panel={}x{} stage={} retried={} rolled-back-to={} passes={} (menubar::compose left its slot untouched — strip::paint declines on a contended SCRATCH or a surface not yet word4. ENABLED restored: an enabled bar with no pixels is a dead band Screen::present_background subtracts forever)",
            pw, ph, staged, retried, bar_was, if retried { 2 } else { 1 }
        );
        return false;
    }

    serial_println!(
        "[orinfurn] ARMED panel={}x{} stage={} rect={:?} bar-was={} retried={} owns_pixels={} table={} -> BAR-ON-GLASS",
        pw, ph, staged, rect, bar_was, retried, painted, live
    );
    // The SHARD menu is reachable from this instant, and that is a SECOND claim, not a restatement:
    // "the bar is painted" and "the bar is live" are two sentences and a capture should not have to
    // infer the second. `crystal::press_at` hit-tests `menubar::crystal_corner_abs` (FITTS-CORNER —
    // the bar's whole upper-left corner cell, not just the glyph), and `wc_click_route`'s furniture
    // arm calls `strip::press_route` ahead of every window arm on an image carrying `desktop_firmware`, which
    // `orinfurn` implies. ⚠ UNFLOWN: no Orin has pressed it. The falsifier is an
    // `[orinclick] edge=press … at (x,y)` INSIDE the corner rect printed here that still ends
    // `-> RAISED` or `MISS-SHELL` — the press reaching a window or the desktop means the menu band
    // did not consume it, which is the bar not being live however painted it looks.
    serial_println!(
        "[orinfurn] crystal LIVE corner={:?} — press the crystal for the SHARD menu (About is real on aarch64; Sleep/Restart/Shut Down print their honest unimplemented lines — no PSCI wiring on this track). UNFLOWN on Orin metal",
        menubar::crystal_corner_abs(pw, ph)
    );
    true
}

// ── ORIN-STKDEPTH ───────────────────────────────────────────────────────────────────────────────
// ⚠ APPEND-ONLY FILE TAIL, and it is the LAST block in the file. Every item below is `#[cfg]`-erased
// in any build without `orinfurn` (and on every arch but aarch64), and nothing exists beneath it to
// be renumbered, so the knob-off jetson image's panic `Location` records are untouched — the
// orinclick line-neutrality rule. The one call site outside this block is a `#[cfg]`-erased append to
// an existing line in `kernel_main`.
//
// **WHAT THIS PUBLISHES: A BOOT-CORE STACK DEPTH, IN BYTES. NOT HEADROOM.** Say it that way on the
// wire and in review, because the two are not the same claim and only one of them is available on
// this board.
//
// THE MEASUREMENT. Two reads of SP, taken in the same frame chain, subtracted:
//   * the ANCHOR, `tegra_stk_anchor()`, on `kernel_main`'s `bootpace::record("entry")` line — the
//     earliest stable point on the terminus path, before any subsystem exists;
//   * the SEAM read, beside `tegra_desk_furn`'s unconditional `[orinfurn] arm` line, i.e. inside the
//     deepest frame the desktop-furniture rung reaches on the boot core's own stack.
// The chain between them is `kernel_main -> tegra_early_stop -> tegra_desk_furn`, and the difference
// is exactly the stack those frames plus their spilled locals have CONSUMED at that instant. It needs
// no linker symbol, no `Task`, no poison fill and no memory map — three instructions and a
// subtraction. `#[inline(always)]` puts the anchor's read in `kernel_main`'s own frame; were it ever
// out-of-lined the anchor would sit one leaf frame lower and the printed depth would be SMALLER, so
// the number is a floor with respect to its anchor and can never overstate.
//
// WHY THIS EXISTS AT ALL. `docs/dev/OS/08_VIDEO/orin-desktop.md` §3.12 records that §5.2's stop-line
// asks for `[u7stk]` evidence that is "structurally unsatisfiable at the terminus". That is exactly
// right about `sched::stk_probe` — it loads `SCHED[cpu].current` and returns early when it is null
// (`arch/aarch64/sched.rs`, the guard right after its own `mov {}, sp`), which is every rung on the
// terminus line, because the boot core has not yet entered `run_capstone_boot_core` and owns no
// `Task`. It is OVERSTATED about the machine: `stk_probe` needs a `Task` only for the BOUNDS it
// prints (`low`/`top`/`hw`/`headroom`), not for the SP read, and this file already contains that same
// three-instruction read twice — `arch/aarch64/mmu_tegra.rs`'s pre-switch PC/SP GiB check and
// `stk_probe`'s own first statement. A DEPTH does not need bounds. So the depth number is takeable
// here and is taken; the stop-line's other half is not, and this block does not pretend otherwise.
//
// ⚠ WHY THERE IS NO HEADROOM NUMBER, and why inventing one would be worse than the silence it
// replaces. Headroom is `depth` measured against the stack's far end, and on the Orin's UEFI path
// nothing in this kernel knows where that end is:
//   * the boot stack is the FIRMWARE'S and is never switched. `crates/bootloader` calls the kernel
//     entry as an ordinary function on the UEFI stack; `arch/aarch64/mmu_tegra.rs` states in its own
//     EL1-twin argument that "the boot stack is never switched on this path"; and the JM6 drop in
//     `arch/aarch64/boot_tegra.rs` does `mov x0, sp; msr sp_el1, x0` precisely so SP is CONTINUOUS
//     across EL2 -> EL1 (which is also what makes the two reads above subtractable at all).
//   * there is no `__stack_top` on this link. The symbol DOES exist in this tree — `pi-baremetal.ld`
//     defines it and the `baremetal` `_start` in this file sets SP from it — but that is the Pi's
//     bare-metal image. The Orin builds through `aarch64-unaos.json`, which names no linker script,
//     so no such symbol is present in the jetson image and no `extern` could resolve to one.
//   * the one runtime description of the region is gone by the time anything could ask. The UEFI
//     `MemoryRegion` slice that would say how far the firmware stack extends is consumed with
//     `boot_info` by `arch::memory::init` on `tegra_early_stop`'s heap line, long before the terminus.
// So: a depth is a measurement, a headroom would be a guess wearing a measurement's clothes. The gap
// is stated on the wire as well as here, so a capture cannot be read as clearing §5.2 by itself.
#[cfg(all(target_arch = "aarch64", feature = "orinfurn"))]
static ORINSTK_ANCHOR_SP: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// ORIN-STKDEPTH — latch SP at the caller's frame. `#[inline(always)]` is load-bearing, not cosmetic:
/// the read must land in `kernel_main`'s frame, not in a callee's. Zero is the "never ran" value and
/// the seam treats it as such, so a build that somehow reached the seam without the anchor prints
/// `DEPTH-UNAVAILABLE` rather than a number derived from a null.
#[cfg(all(target_arch = "aarch64", feature = "orinfurn"))]
#[inline(always)]
fn tegra_stk_anchor() {
    let sp: u64;
    unsafe {
        core::arch::asm!("mov {}, sp", out(reg) sp, options(nomem, nostack, preserves_flags));
    }
    ORINSTK_ANCHOR_SP.store(sp, core::sync::atomic::Ordering::Relaxed);
}

// ============================== ORINRENDER — the Orin's render pass ==============================
// Rung 5's owed core (orin-desktop.md §3.6). DEFAULT OFF. The full argument for why the Pi's
// `render_service` is not portable, and why this grows the existing pump's other half instead of
// adding a second event loop, is on the `orinrender` feature in Cargo.toml. Short form: both of the
// Pi service's transports are `baremetal`-gated and `baremetal` + `tegra` is a hard `compile_error!`;
// its blocking `recv()` would park forever because `run_capstone_boot_core` has no sleeper drain;
// and `jd2_console_pump` already owns the input half, so a second drainer would steal its events.

/// One-shot latch — the seam must never run twice. A second pass would spawn a second `orin-render`
/// task presenting into the same stage (the shell-window mint it once guarded is gone — PAINTPULSE).
#[cfg(all(target_arch = "aarch64", feature = "orinrender"))]
static ORINRENDER_ARMED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// ORINRENDER — evaluate the floors on the tegra terminus and spawn the render pass.
///
/// Returns whether the service was spawned. Every decline is NAMED on the wire: a silent `false`
/// must never be indistinguishable from "the seam was never called", which is the ambiguity that
/// made `orinfurn`'s missing bar read as a mystery rather than as a refusal.
#[cfg(all(target_arch = "aarch64", feature = "orinrender"))]
fn tegra_render_arm() -> bool {
    use core::sync::atomic::Ordering;

    // THE ENTRY LINE, before anything can decline. The sibling knobs are printed because this rung's
    // usefulness is a function of them — without `orinconwin` there is no console window beside the
    // shell window this mints — and printed as `cfg!()` reads rather than as literals, so the line is
    // falsifiable rather than trusted. That distinction is not pedantry here: `orinconwin` shipped a
    // `live=LIVE` that was a compile-time literal and would have printed LIVE whatever the route did.
    serial_println!(
        "[orinrender] arm conwin={} furn={} tenant={} click={} desk={} (ORIN-RENDER entered on the tegra terminus; §5.2 UNCROSSED — desktop_firmware::activate is NOT called)",
        cfg!(feature = "orinconwin") as u8,
        cfg!(feature = "orinfurn") as u8,
        cfg!(feature = "orintenant") as u8,
        cfg!(feature = "orinclick") as u8,
        cfg!(feature = "orindesk") as u8
    );

    if ORINRENDER_ARMED.swap(true, Ordering::AcqRel) {
        serial_println!("[orinrender] REFUSE reason=already-armed (the seam is one-shot; a second spawn would mint a second shell window and orphan the first row's heap store)");
        return false;
    }

    // FLOOR 1 — THE PANEL, read off the surface the compositor composites onto rather than assumed.
    // Copy the info out and drop `WRITER` immediately: nothing below may hold it across a `wm` call,
    // which is ORIN-WM1's WRITER/TABLE acquisition-order rule. A headless Orin (no DTB
    // `simple-framebuffer` handoff, so JD1 seeded no scanout) has no geometry to mint against.
    let info = {
        let fb = *unaos_kernel::video::WRITER.lock();
        if !fb.is_ready() {
            serial_println!("[orinrender] REFUSE reason=no-panel (headless boot — JD1 seeded no scanout; the shell window's geometry and the present that paints it are both functions of panel geometry)");
            return false;
        }
        fb.info()
    };
    let (pw, ph) = (info.width, info.height);

    // FLOOR 2 — THE STAGING BUFFER, DESKSEAM's reason restated: `wm`'s staged presents run inside
    // `SYS_WIN_PRESENT`'s IRQ mask, so a stage that GREW on the pass would be a masked acquisition of
    // the global heap `Mutex` — the F1-F5 defect family. The tree's only `reserve_stage` caller on a
    // normal boot is `video::init_panel`, which the tegra path never reaches (it seeds `WRITER` by
    // hand instead), so a knob-solo `orinrender` image — no `orindesk`/`orinfurn`/`orintenant` seam
    // ahead of it to have made this call — would grow the STAGE `Vec` lazily inside the mask on its
    // first present. Grow-only and idempotent, so it is safe beside the identical calls the sibling
    // seams make. FAILS when the 48 MiB aarch64 heap cannot spare entry 0 at all.
    let staged = unaos_kernel::video::wm::reserve_stage(&info);
    if staged == 0 {
        serial_println!("[orinrender] REFUSE reason=stage-unreserved panel={}x{} stage=0 (wm::reserve_stage got nothing for entry 0 — every present this pass drives would grow its buffer under SYS_WIN_PRESENT's IRQ mask, the F1-F5 shape)", pw, ph);
        return false;
    }

    // THE GAP THIS RUNG EXISTS TO CLOSE. `desktop_firmware::armed()` is false for the entire Orin
    // boot: its only setter is `arm()`, whose one call site is inside `u7_launcher`, spawned from the
    // `kernel_main` body that tegra never reaches (`tegra_early_stop` is `-> !`). So on the Orin
    // `open_shell_window`'s guard has never once been true, and the minting code has sat compiled and
    // unreachable. `arm()` is a latch store plus one serial line — it is NOT `activate()`, and the
    // cascade §5.2 stops is not driven here. Armed BEFORE the spawn so the service's first pass can
    // mint rather than idling a full pass waiting for a latch nothing else on this board will set.
    unaos_kernel::video::desktop_firmware::arm();
    // PAINTPULSE — the SAME gap, one latch over. `pulsewin::service()` below opens its window only on
    // `pulsewin::ARMED`, whose sole setter before this line was `desktop_firmware::activate()`
    // (`desktop_firmware.rs`, the `[pidesk] pulse-window ARMED` site) — the call §5.2 forbids on tegra.
    // So the service pass ran every pass and could never open. `arm()` is the latch store and nothing
    // else; the open itself stays where the Pi put it, in the render pass, on the first live sample.
    unaos_kernel::video::pulsewin::arm();

    // PLACEMENT IS CHOSEN, NOT CONVENIENT — §5.2's rule, from two seats on the same day (rmbp's steal
    // handed composite work to the busiest core; pi co-pinned `usb-pump` with `input` and destroyed
    // the only redundancy in its service set). cpu 0 is not a convenience here, it is the only legal
    // choice: cores 1-5 are online and dispatching but are still at EL2 (only the boot core drops to
    // EL1), and `run_capstone_boot_core`'s loop is the only dispatcher that will ever run this task.
    // Explicit pin, never CPU_AUTO — `ONLINE_MASK[0]` is false on the Orin, so an auto placement would
    // be decided by a filter that does not know this core exists.
    //
    // STACKSEED — SIZED, NOT BLANKET. The render1 flight traversed the blanket 16 KiB task stack's
    // redzone on pass ~1: this task's pass calls into `wm` (`pulsewin::service`, `ui_status::tick`,
    // the `Screen` flush with its WC-I occluder walk), and `composite_inner`'s aarch64 stack
    // exhaustion is already on the ledger (occ62). Same size and same shape as the Pi's
    // `U7_LAUNCH_STACK_SIZE` (and its `RENDER_STACK_SIZE` / `PUMP_PATH_STACK_SIZE` siblings on
    // `hw-pi4`): one task pays 16 KiB of heap, `TASK_STACK_SIZE` stays what it is for every other
    // task. `spawn_stack` is ungated; `spawn_prio_stack` is `baremetal`-gated and not usable on
    // tegra. The `[u7stk]` gauge SATURATES (`hw=len headroom=0` is a lower bound, never a depth), so
    // the flight's reading says "at least 16 KiB", not "16 KiB" — 32 KiB is the next power of two,
    // and the next `[u7stk]` line is what says whether it fits.
    const RENDER_STACK_SIZE: usize = 32 * 1024;
    let tid = unaos_kernel::arch::sched::spawn_stack(
        "orin-render",
        orin_render_service,
        0,
        0,
        RENDER_STACK_SIZE,
    );
    serial_println!(
        "[orinrender] spawned tid={} cpu=0 pinned=1 discipline=cooperative -> RENDER-ARMED (the terminus dispatches it; a spawn only pushes to the run queue)",
        tid
    );
    true
}

/// The Orin's render pass. Cooperative, pinned to cpu 0, and it NEVER SLEEPS.
///
/// `run_capstone_boot_core` has no `drain_due_sleepers` and never sets `SCHED_ACTIVE`, so a task on
/// this core that sleeps parks in `SLEEPERS[0]` and is never woken. `yield_now` is the only legal
/// blocking discipline here, which is the same reason `jd2_console_pump` busy-polls.
///
/// IT DOES NOT DRAIN EVENTS. `jd2_console_pump` owns `pal::EVENT_QUEUE` and feeds `handle_key`; a
/// second drainer would steal keystrokes from the shell. This task owns the PAINT half only.
#[cfg(all(target_arch = "aarch64", feature = "orinrender"))]
fn orin_render_service(_: usize) {
    // `render`/`width`/`height`/`clear_screen` are trait methods on `GneissPal`, not inherent ones.
    use unaos_kernel::pal::GneissPal;
    use unaos_kernel::video::wm;

    let front_fb = *unaos_kernel::video::WRITER.lock();
    let info = front_fb.info();
    let (pw, ph) = (info.width, info.height);
    let mut screen = unaos_kernel::video::Screen::new(front_fb);
    // STACKSEED — THE BACK BUFFER IS SEEDED THE DESKTOP COLOUR BEFORE ANYTHING CAN PRESENT IT.
    // `Screen::new` allocates its back store `vec![0u8; len]` and arms FULL-PANEL damage, so the
    // first `pal.render` below would publish a whole panel of black that nothing ever painted. Until
    // a1cf4900 the mint arm's `clear_screen` was seeding it by accident — on a routed board that arm
    // never fires, so its deletion was right and this is the seed's proper home: once, before the
    // loop. It is the same fill `screen::adopt_desktop_bg` would have `Screen::new` perform, done on
    // THIS screen only rather than through that latch, because the latch is global and this board
    // has a second reader: `video::witness::run` (the shell's `tste` leg `video.present`, compiled
    // unconditionally) builds a `Screen` over a HEAP buffer and asserts its baseline flush left the
    // front all-zero — the same reason the Pi's desktop path (`desktop_firmware` step 1b) declines to
    // arm it on aarch64. A cached-RAM fill, never a panel read-back; `mark_full` re-arms the damage
    // `Screen::new` already set, so no extra present. The flush's WC-I occluder walk keeps the fill
    // off every `wm` window's box, so the routed console window survives the first present, and the
    // colour is the one `wm::erase` already paints into vacated boxes on this arch.
    screen.fill_screen(wm::DESKTOP_BG);
    let mut pal = unaos_kernel::pal::TargetPal::new(&mut screen);

    // PAINTPULSE — no shell window is minted on this board any more (both arms below decline), so
    // there is no heap store to keep alive and no row id to learn; `shell_id` stays `WIN_NONE` and the
    // census prints it as `win=0`, which is the truth rather than a row nothing paints.
    let shell_id = wm::WIN_NONE;
    // One decline is final. The arms below name their reason on the wire; repeating it every pass
    // would be a serial flood.
    let mut shell_declined = false;
    let mut passes: u64 = 0;
    let mut presents: u64 = 0;
    // The census cadence rides CNTPCT (free-running, EL-independent — the JD3 timerless mechanism;
    // this post-drop EL1 core has no timer IRQ), the same shape as `jd2_console_pump`'s sweep.
    // A PASS COUNT is not a rate limit on a busy-poll loop: `passes % 20000` printed one line per
    // ~4 ms and was 82.2% of the post-terminus capture. ~1 s of wall time per line; seeded one
    // period in the past so the first pass still prints immediately.
    let census_ticks: u64 = {
        let f = unaos_kernel::arch::timer::cntfrq();
        if f == 0 { 62_500_000 } else { f }
    };
    let mut last_census = unaos_kernel::arch::timer::cntpct().wrapping_sub(census_ticks);

    loop {
        passes += 1;
        let mut dirty = false;

        // THE DECLINE, once. Still guarded on `armed()` rather than on having called `arm()` above,
        // because the guard is what `open_shell_window` itself reads and a rung that checks a different
        // fact than the one that gates it is the defect class this branch keeps paying for.
        if shell_id == wm::WIN_NONE
            && !shell_declined
            && unaos_kernel::video::desktop_firmware::armed()
        {
            // FLIGHT-CORRECTED 2026-09-01 (render1 boot, `win=3 att=0 dout=0 dkpx=0`). The first
            // flight minted the window and it was an EMPTY RECTANGLE for 71.5M passes. The reason is
            // not a missing paint — it is that ON THIS BOARD THE WINDOW IS REDUNDANT. `orinconwin`
            // already routes the live console into its own `wm` row (`win=2 … route=true`), so the
            // Orin's shell is ALREADY in a window. The Pi needs `open_shell_window` because the Pi
            // has no `orinconwin`; porting the mint here minted a second, contentless row above the
            // real one. Ask the same fact `orinconwin` sets, not a `cfg!()` proxy for it: an image
            // can carry the feature and still have the route decline at its ordering rule.
            if unaos_kernel::video::fbcon::console_is_routed() {
                shell_declined = true;
                serial_println!("[orinrender] DECLINE reason=console-already-windowed (fbcon::console_is_routed()=true — orinconwin owns the live console in its own wm row, so a shell window here is a second contentless row stacked above the real one; the render PASS below is what this rung contributes on a routed board)");
            } else {
                // PAINTPULSE — the knob-solo arm used to mint here, and the window it minted was EMPTY
                // BY CONSTRUCTION: `open_shell_window`'s surface came back as `_surf_fb` and was
                // dropped on the floor, nothing ever drew into the store, and the row composited a
                // `DESKTOP_BG` fill for the life of the boot (`att=0 comp>0`). The Pi's painter is
                // seven lines, and every symbol in it compiles here — but its window is LIVE only
                // because the Pi's render task also drains `EVENT_QUEUE` into that console. On this
                // board `jd2_console_pump` owns the drain and its own `Console`, and this task's
                // contract (see its doc) forbids a second drainer; a port of the paint alone would
                // mint one frame of a prompt no keystroke can reach. That is a picture of a shell,
                // not a shell, and a worse lie than the empty box. So: no painter, no mint, said so.
                shell_declined = true;
                serial_println!(
                    "[orinrender] DECLINE reason=no-painter panel={}x{} (fbcon::console_is_routed()=false — the shell on this image is jd2_console_pump's console on the panel; this task owns no drainer for a windowed shell, so it mints no window it cannot paint; the strip pass below is what this rung contributes)",
                    pw, ph
                );
            }
        }

        // The pulse window's own service pass. Compiled on tegra under `desktop_firmware` and, until
        // this rung, called by nothing on this board; armed by `tegra_render_arm` (PAINTPULSE), so the
        // open arm inside it can fire on the first pass `ui_status::loads` reports a live instrument.
        unaos_kernel::video::pulsewin::service();

        // The status strip's ~1 Hz repaint. `tick` returns whether it changed anything, so a quiet
        // pass costs one compare and no present.
        dirty |= unaos_kernel::ui_status::tick(&mut pal);

        // AT MOST ONE PRESENT PER PASS, and only when something changed. The Pi service measured a
        // 96-100% core peg before it took this rule; on this board the cost lands on cpu 0, which is
        // also the only core that can dispatch `jd2_console_pump`.
        if dirty {
            pal.render();
            presents += 1;
            // STACKSEED — the ONE real stack reading this task gets. `stk_probe` reads the CURRENT
            // task's bounds and its poison high-water, so calling it here yields orin-render's own
            // numbers, and it is placed AFTER the first present because the present (flush + the
            // WC-I occluder walk) is this pass's deepest call chain — a probe before it would read a
            // shallower stack. Passes 1 and 2: `ui_status::tick`'s arming call always returns dirty
            // ("first frame must paint the bars"), so pass 1 is the first present by construction;
            // but `pulsewin::service()` runs BEFORE `tick` each pass and `loads` is 0 until tick has
            // armed, so the pulse window's `wm::create_at` + composite — this task's deepest chain —
            // lands on pass 2, and a pass-1-only gauge would exclude it. Two lines are what the
            // wire's lossy polled UART can be asked to carry (REVIEW finding, orin 13). Every other
            // `[u7stk]` site in the tree sits in `u7_launcher`, below the `-> !` terminus this board
            // never returns from, so without this line the jetson image has NO stack gauge at all.
            // ⚠ The reading SATURATES: `hw=len headroom=0` is a lower bound, never a depth. Witness-
            // gated as the probe itself is; knob-off the image is unchanged.
            #[cfg(feature = "witness")]
            if passes == 1 || passes == 2 {
                unaos_kernel::arch::sched::stk_probe(if passes == 1 { "orin-render:pass1" } else { "orin-render:pass2" });
            }
        }

        // Rate-limited liveness. This task is a SINGLETON ROLE and this tree has no re-home path —
        // §5.2: `steal_ok` is false for every explicitly-pinned task, so if this dies nothing inherits
        // the paint. Pi boot 11 printed `verdict=LIVE` one line after the exception that killed cpu 3,
        // so the counter is printed rather than a bare "alive".
        if unaos_kernel::arch::timer::cntpct().wrapping_sub(last_census) >= census_ticks {
            last_census = unaos_kernel::arch::timer::cntpct();
            serial_println!(
                "[orinrender] census passes={} presents={} win={} declined={} -> RENDER-LIVE",
                passes, presents, shell_id, shell_declined as u8
            );
        }

        // Cooperative yield — never `sleep_ticks`. See this function's doc.
        unaos_kernel::arch::sched::yield_now();
    }
}
