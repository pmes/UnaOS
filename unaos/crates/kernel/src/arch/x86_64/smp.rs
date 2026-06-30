// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// SMP application-processor (AP) bring-up.
//
// APs come out of reset (and out of a SIPI) in 16-bit real mode at CS:IP = (vector<<8):0, i.e.
// physical address vector*0x1000. There is no shortcut on x86: to run 64-bit Rust an AP must
// walk real -> protected -> long mode itself. So we copy a small trampoline to a low,
// identity-mapped page (0x8000) and kick each AP with the architectural INIT-SIPI-SIPI sequence.
//
// The trampoline is fully position-dependent on 0x8000 (it bakes that base into every absolute
// reference as `0x8000 + (label - start)`), which means it contains NO relocations and can be
// copied byte-for-byte. It sets up a temporary GDT (32-bit + 64-bit descriptors), enables PAE +
// the BSP's page tables (CR3) + long mode, and jumps to `ap_entry` on a per-AP stack. From there
// each AP loads its own per-CPU GDT/TSS (gdt::init_cpu), the shared IDT, and its own local APIC,
// then idles — the BSP keeps driving xHCI/console/storage.

use core::sync::atomic::{AtomicU32, Ordering};

use alloc::vec::Vec;
use spin::Mutex;

use crate::arch::{acpi, apic, gdt, interrupts, percpu, sched};

/// Physical page the trampoline is copied to and the AP starts executing at. Must be page-aligned
/// and < 1 MiB (the SIPI vector is 8 bits: start address = vector << 12). 0x8000 is free
/// conventional RAM in our UEFI memory map once boot services have exited.
const TRAMPOLINE_ADDR: usize = 0x8000;
/// SIPI vector byte that selects `TRAMPOLINE_ADDR` (0x8000 >> 12 = 0x08).
const SIPI_VECTOR: u8 = (TRAMPOLINE_ADDR >> 12) as u8;

/// Per-AP kernel stack size (the BSP keeps its UEFI boot stack; only APs need new ones).
const AP_STACK_SIZE: usize = 4096 * 4; // 16 KiB

/// 16-byte-aligned AP kernel stacks in `.bss`, one per logical CPU (index 0 / the BSP is unused).
/// Static, not heap: APs touch no shared allocator state during bring-up.
#[repr(C, align(16))]
struct ApStack([u8; AP_STACK_SIZE]);
static mut AP_STACKS: [ApStack; gdt::MAX_CPUS] =
    [const { ApStack([0; AP_STACK_SIZE]) }; gdt::MAX_CPUS];

/// Count of APs that have reached `ap_entry` and finished their own bring-up. The BSP waits on
/// this between SIPIs so the shared trampoline handoff slot (stack/index) is reused safely.
static AP_ONLINE: AtomicU32 = AtomicU32::new(0);

/// Logical indices of the APs that came online, published by `start_aps` for the scheduler to
/// spawn work onto. Written once on the BSP after bring-up; read on the BSP.
static ONLINE_APS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// Snapshot of the online application-processor logical indices (excludes the BSP).
pub fn online_aps() -> Vec<usize> {
    ONLINE_APS.lock().clone()
}

// The real-mode -> long-mode trampoline. AT&T syntax; see the module comment for the design.
// Every absolute reference is `TRAMP + (label - ap_trampoline_start)` so the assembled bytes
// carry no relocations and are valid only after being copied to TRAMP (0x8000).
core::arch::global_asm!(
    r#"
.section .rodata
.balign 16
.code16
.global ap_trampoline_start
ap_trampoline_start:
    cli
    cld
    xorw   %ax, %ax
    movw   %ax, %ds
    movw   %ax, %es
    movw   %ax, %ss
    # Load the temporary GDT (absolute address, ds = 0).
    lgdtl  0x8000 + ap_gdt_ptr - ap_trampoline_start
    # Enter protected mode.
    movl   %cr0, %eax
    orl    $1, %eax
    movl   %eax, %cr0
    # Far jump into the 32-bit code segment (selector 0x08).
    ljmpl  $0x08, $(0x8000 + ap_pm_entry - ap_trampoline_start)

.code32
ap_pm_entry:
    movw   $0x10, %ax            # 32-bit data selector
    movw   %ax, %ds
    movw   %ax, %es
    movw   %ax, %ss
    movw   %ax, %fs
    movw   %ax, %gs
    # Enable PAE (CR4.PAE, bit 5) — required for long mode.
    movl   %cr4, %eax
    orl    $0x20, %eax
    movl   %eax, %cr4
    # Load the BSP's PML4 (shared identity map) into CR3.
    movl   $(0x8000 + ap_param_cr3 - ap_trampoline_start), %ecx
    movl   (%ecx), %eax
    movl   %eax, %cr3
    # Set EFER.LME (long mode enable, bit 8).
    movl   $0xC0000080, %ecx
    rdmsr
    orl    $0x100, %eax
    wrmsr
    # Enable paging + protection (CR0.PG | CR0.PE) — activates long mode.
    movl   %cr0, %eax
    orl    $0x80000001, %eax
    movl   %eax, %cr0
    # Far jump into the 64-bit code segment (selector 0x18).
    ljmpl  $0x18, $(0x8000 + ap_lm_entry - ap_trampoline_start)

.code64
ap_lm_entry:
    xorl   %eax, %eax
    movw   %ax, %ds
    movw   %ax, %es
    movw   %ax, %ss
    movw   %ax, %fs
    movw   %ax, %gs
    # rsp = *(param_stack); rdi = *(param_index); jump to *(param_entry).
    movl   $(0x8000 + ap_param_stack - ap_trampoline_start), %ecx
    movq   (%rcx), %rsp
    movl   $(0x8000 + ap_param_index - ap_trampoline_start), %ecx
    movq   (%rcx), %rdi
    movl   $(0x8000 + ap_param_entry - ap_trampoline_start), %ecx
    movq   (%rcx), %rax
    jmpq   *%rax

# --- Temporary GDT: null, 32-bit code (0x08), 32-bit data (0x10), 64-bit code (0x18). ---
.balign 8
ap_gdt:
    .quad 0x0000000000000000
    .quad 0x00CF9A000000FFFF
    .quad 0x00CF92000000FFFF
    .quad 0x00AF9A000000FFFF
ap_gdt_ptr:
    .word ap_gdt_ptr - ap_gdt - 1
    .long 0x8000 + ap_gdt - ap_trampoline_start

# --- Parameter block, patched by the BSP before each SIPI. ---
.balign 8
.global ap_param_cr3
.global ap_param_entry
.global ap_param_stack
.global ap_param_index
ap_param_cr3:    .quad 0
ap_param_entry:  .quad 0
ap_param_stack:  .quad 0
ap_param_index:  .quad 0
.global ap_trampoline_end
ap_trampoline_end:
"#,
    options(att_syntax)
);

unsafe extern "C" {
    static ap_trampoline_start: u8;
    static ap_trampoline_end: u8;
    static ap_param_cr3: u8;
    static ap_param_entry: u8;
    static ap_param_stack: u8;
    static ap_param_index: u8;
}

/// 64-bit entry for a freshly long-moded AP. Runs on this AP's own stack with `cpu_index` passed
/// in rdi by the trampoline. Brings the AP fully online: its own per-CPU GDT/TSS, the shared IDT,
/// and its own local APIC (x2APIC + timer), then idles. Never touches xHCI/console/heap — those
/// stay BSP-owned.
#[unsafe(no_mangle)]
pub extern "C" fn ap_entry(cpu_index: u64) -> ! {
    let idx = cpu_index as usize;
    gdt::init_cpu(idx);
    interrupts::init_idt();
    apic::init();

    let apic_id = apic::apic_id_u32();
    // Per-CPU data + GS base before enabling interrupts, so this AP's timer/IPI handlers can
    // resolve `this_cpu()`.
    percpu::init_cpu(idx, apic_id);

    AP_ONLINE.fetch_add(1, Ordering::SeqCst);
    serial_println!("SMP: AP {} online (apic id {}).", idx, apic_id);

    x86_64::instructions::interrupts::enable();
    // Wait until the BSP has run SMP verification and turned scheduling on, then run this AP's
    // scheduler loop forever (replacing the old idle `hlt_loop`). The BSP keeps driving
    // xHCI/console/storage; APs run scheduled kernel threads.
    sched::wait_and_run();
}

/// Patch one 8-byte field of the (already-copied) trampoline parameter block at TRAMPOLINE_ADDR.
/// `param` is the link-time symbol; its offset from `ap_trampoline_start` is the same at 0x8000.
unsafe fn patch_param(param: *const u8, val: u64) {
    let start = &raw const ap_trampoline_start as usize;
    let off = param as usize - start;
    unsafe { core::ptr::write_volatile((TRAMPOLINE_ADDR + off) as *mut u64, val) };
}

/// Crude bounded busy-wait. We don't have a calibrated microsecond clock yet (the APIC timer is
/// uncalibrated), but exact timing isn't needed: the INIT/SIPI delays only have to let the AP
/// latch each command, and the real synchronisation is the `AP_ONLINE` handshake below.
fn spin_delay(iterations: u64) {
    for _ in 0..iterations {
        core::hint::spin_loop();
    }
}

/// Architectural INIT-SIPI-SIPI to start the AP with the given APIC id.
fn init_sipi_sipi(apic_id: u32) {
    const INIT: u32 = 0x0000_4500; // delivery mode 101 (INIT), level assert
    let sipi: u32 = 0x0000_4600 | SIPI_VECTOR as u32; // delivery mode 110 (Startup), assert, vector

    apic::send_ipi(apic_id, INIT);
    spin_delay(2_000_000); // ~INIT settle (>=10ms on real HW; generous here)
    apic::send_ipi(apic_id, sipi);
    spin_delay(100_000); // ~200us between SIPIs
    apic::send_ipi(apic_id, sipi);
}

/// Bring up every application processor reported by ACPI. Called once on the BSP after ACPI
/// discovery. APs are started one at a time (the trampoline's stack/index handoff slot is shared)
/// and the BSP waits for each to report online before starting the next. Degrades cleanly:
/// missing topology, or an AP that never checks in, just leaves that core offline.
pub fn start_aps() {
    let topo = match acpi::topology() {
        Some(t) => t,
        None => {
            serial_println!("SMP: no ACPI topology; staying uniprocessor.");
            return;
        }
    };
    let apic_ids = topo.apic_ids();
    if apic_ids.len() <= 1 {
        serial_println!("SMP: 1 CPU; no APs to start.");
        return;
    }

    let bsp_id = apic::apic_id_u32();

    // Validate the fixed trampoline page against the real UEFI map. 0x8000 is free conventional RAM
    // on QEMU/OVMF, but Apple EFI fragments low memory and may mark it Reserved/Bootloader — in
    // which case writing the trampoline there (or the AP executing it) is unsound and APs may
    // silently fail to start. This turns that into a visible breadcrumb on the (serial-less) Mac;
    // the fix if it fires is to scan the map for a free low page and retarget the SIPI vector.
    if !crate::arch::memory::region_is_usable(TRAMPOLINE_ADDR as u64, 0x1000) {
        serial_println!(
            "SMP: WARNING: trampoline page {:#x} is NOT Usable in the UEFI map — APs may fail to \
             start (firmware may reclaim/clobber it).",
            TRAMPOLINE_ADDR
        );
    }

    // Copy the trampoline to its low page and patch the fields common to every AP.
    let cr3 = x86_64::registers::control::Cr3::read().0.start_address().as_u64();
    unsafe {
        let start = &raw const ap_trampoline_start as *const u8;
        let end = &raw const ap_trampoline_end as *const u8;
        let len = end as usize - start as usize;
        core::ptr::copy_nonoverlapping(start, TRAMPOLINE_ADDR as *mut u8, len);
        patch_param(&raw const ap_param_cr3, cr3);
        patch_param(&raw const ap_param_entry, ap_entry as *const () as u64);
    }
    serial_println!(
        "SMP: starting APs (trampoline @ {:#x}, cr3 {:#x}, {} CPUs)...",
        TRAMPOLINE_ADDR,
        cr3,
        apic_ids.len()
    );

    // Logical indices of APs that reported online (BSP is 0, handled separately).
    let mut online_aps: [usize; gdt::MAX_CPUS] = [0; gdt::MAX_CPUS];
    let mut n_online = 0usize;

    let mut index = 1usize; // logical CPU 0 is the BSP
    for &id in apic_ids {
        if id == bsp_id {
            continue;
        }
        if index >= gdt::MAX_CPUS {
            serial_println!("SMP: MAX_CPUS reached; skipping remaining APs.");
            break;
        }

        // Per-AP handoff: 16-byte-aligned stack top minus 8 (SysV ABI expects rsp%16==8 at a
        // function entry reached via call; we arrive via jmp, so bias by 8) and the logical index.
        let stack_top = unsafe {
            let base = &raw const AP_STACKS[index] as usize;
            (base + AP_STACK_SIZE - 8) as u64
        };
        unsafe {
            patch_param(&raw const ap_param_stack, stack_top);
            patch_param(&raw const ap_param_index, index as u64);
        }

        let target = AP_ONLINE.load(Ordering::SeqCst) + 1;
        init_sipi_sipi(id);

        // Wait (bounded) for this AP to report in before reusing the handoff slot.
        let mut came_online = false;
        for _ in 0..50_000_000u64 {
            if AP_ONLINE.load(Ordering::SeqCst) >= target {
                came_online = true;
                break;
            }
            core::hint::spin_loop();
        }
        if came_online {
            online_aps[n_online] = index;
            n_online += 1;
        } else {
            serial_println!("SMP: WARNING: AP apic id {} did not come online (timeout).", id);
        }

        index += 1;
    }

    serial_println!(
        "SMP: bring-up complete — {} of {} CPUs online (incl. BSP).",
        AP_ONLINE.load(Ordering::SeqCst) + 1,
        apic_ids.len()
    );

    // Publish the online AP indices so the scheduler can spawn work onto exactly the cores that
    // actually came up (not just "1..cpu_count").
    *ONLINE_APS.lock() = online_aps[..n_online].to_vec();

    verify_smp(&online_aps[..n_online]);
}

/// Post-bring-up smoke test: prove the SMP plumbing actually works. Confirms every core's local
/// APIC timer is ticking (each CPU has its own per-CPU tick counter) and that each AP answers a
/// fixed IPI (the cross-CPU wakeup primitive a scheduler will use).
fn verify_smp(online_aps: &[usize]) {
    // Let real time pass by waiting for the BSP's own timer to advance several ticks, so the APs'
    // (earlier-armed) timers have certainly ticked too.
    let bsp_ticks = percpu::this_cpu();
    let base = bsp_ticks.ticks.load(Ordering::Relaxed);
    for _ in 0..200_000_000u64 {
        if bsp_ticks.ticks.load(Ordering::Relaxed) >= base + 5 {
            break;
        }
        core::hint::spin_loop();
    }

    // Per-CPU timer: BSP first, then each online AP.
    serial_println!(
        "SMP: per-CPU timer — cpu 0 (apic {}) ticks={}",
        bsp_ticks.apic_id,
        bsp_ticks.ticks.load(Ordering::Relaxed)
    );
    for &i in online_aps {
        if let Some(c) = percpu::cpu(i) {
            serial_println!(
                "SMP: per-CPU timer — cpu {} (apic {}) ticks={}",
                i,
                c.apic_id,
                c.ticks.load(Ordering::Relaxed)
            );
        }
    }

    // IPI round-trip: knock each AP with a fixed IPI and confirm its handler ran.
    let icr_low = 0x0000_4000 | interrupts::IPI_VECTOR as u32; // fixed delivery, assert, vector
    for &i in online_aps {
        let Some(c) = percpu::cpu(i) else { continue };
        let before = c.ipis.load(Ordering::SeqCst);
        apic::send_ipi(c.apic_id, icr_low);

        let mut acked = false;
        for _ in 0..20_000_000u64 {
            if c.ipis.load(Ordering::SeqCst) > before {
                acked = true;
                break;
            }
            core::hint::spin_loop();
        }
        serial_println!(
            "SMP: IPI -> cpu {} (apic {}): {}",
            i,
            c.apic_id,
            if acked { "ack" } else { "NO ACK" }
        );
    }
}
