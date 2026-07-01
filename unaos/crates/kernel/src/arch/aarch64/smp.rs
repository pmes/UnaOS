// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// aarch64 SMP bring-up — releasing the parked Cortex-A72 secondary cores on the Raspberry Pi 4.
//
// The GPU firmware (default armstub, no `armstub=` line) starts only core 0 at our kernel; cores
// 1-3 sit in the firmware's spin-table, each in `wfe; ldr xN,[release_addr]; cbz xN,loop; br xN`,
// watching a per-core 64-bit slot in the first page of RAM (core1=0xE0, core2=0xE8, core3=0xF0 —
// from the BCM2711 DTB `cpu-release-addr`). To release a core we write our entry point into its
// slot and `SEV`; it wakes, reads a now-non-zero slot, and branches to us.
//
// Two things make this correct on real silicon (both no-ops in QEMU, which QEMU raspi4b models with
// the *same* spin-table so this exact code is testable there):
//   * The parked core reads its slot with the **MMU off / non-cacheable**, while we write it with
//     the MMU on and RAM cacheable — so our store can sit in cache and never be seen. We must clean
//     the slot to the Point of Coherency (`DC CVAC` + `DSB`) before `SEV`.
//   * A released core arrives in the *same* raw state core 0 did: EL2, MMU OFF, caches off, no
//     stack. So each secondary re-runs the per-core setup (stack, `enable_mmu`, exception vectors)
//     before it can touch a lock or atomic. That's what `_secondary_start` + `__secondary_rust` do.

use super::cache;

/// The Pi 4 is a single cluster of 4 Cortex-A72s (MPIDR Aff0 = 0..3).
const NUM_CORES: usize = 4;

/// Per-core spin-table release slots (physical addresses in the first page), from the BCM2711 DTB
/// `cpu-release-addr`. Index by MPIDR Aff0; slot 0 (0xD8) is core 0's and unused (it's already up).
const RELEASE_ADDR: [usize; NUM_CORES] = [0xD8, 0xE0, 0xE8, 0xF0];

/// One secondary's boot/idle stack (16 KiB… 64 KiB; AArch64 SP must stay 16-aligned, which the
/// type alignment + power-of-two size guarantee). Lives in BSS (zeroed by core 0's `_start`).
const SEC_STACK_SIZE: usize = 0x1_0000; // 64 KiB
#[repr(C, align(16))]
struct SecStack([u8; SEC_STACK_SIZE]);
/// One stack per core, indexed by Aff0. Slot 0 is unused (core 0 has the linker `__stack_top`); the
/// secondaries take slots 1-3. `_secondary_start` computes its own top: base + (core+1)*size.
static mut SECONDARY_STACKS: [SecStack; NUM_CORES] =
    [const { SecStack([0; SEC_STACK_SIZE]) }; NUM_CORES];

// The secondary entry stub. Runs with the MMU OFF at EL2 (x0-x3 = 0 per the spin-table protocol).
// It sets SP to this core's stack top, then tail-calls the Rust entry. Position-independent absolute
// symbol references (adrp/add, and `sym`) resolve to the correct address because the kernel is
// identity-mapped (VA == PA). Kept out of `.text.boot` on purpose — it's reached by address, not by
// being first in the image.
core::arch::global_asm!(
    r#"
    .globl _secondary_start
    _secondary_start:
        mrs   x0, mpidr_el1
        and   x0, x0, #0xff          // x0 = core id (Aff0)
        adrp  x1, {stacks}
        add   x1, x1, #:lo12:{stacks}
        mov   x2, #({size} >> 12)
        lsl   x2, x2, #12            // x2 = SEC_STACK_SIZE
        madd  x3, x0, x2, x1         // x3 = &SECONDARY_STACKS + core*size
        add   x3, x3, x2            // + size  => top of this core's stack
        mov   sp, x3
        bl    {entry}               // __secondary_rust(core)  — never returns
    1:  wfe
        b     1b
    "#,
    stacks = sym SECONDARY_STACKS,
    entry = sym __secondary_rust,
    size = const SEC_STACK_SIZE,
);

unsafe extern "C" {
    fn _secondary_start();
}

/// Rust entry for a released secondary core. Called from `_secondary_start` with the **MMU still
/// off** and this core's stack set. Turns the MMU on FIRST (before any lock/atomic — `serial_println`
/// takes a spinlock), installs this core's exception vectors, then (Milestone 1) parks. Later
/// milestones replace the park with per-core GIC + timer + the scheduler.
#[unsafe(no_mangle)]
extern "C" fn __secondary_rust(core: u64) -> ! {
    // MMU on, using the L1 table the BSP already built (`enable_mmu` touches no per-core state).
    unsafe { super::boot::enable_mmu() };
    // Per-core VBAR_EL2 so a fault on this core is caught rather than jumping to a stale vector.
    super::exceptions::install();

    serial_println!(":: AARCH64 SMP: core {} online (EL2, MMU on) ::", core);

    // Milestone 1: prove the core is alive, then idle. IRQ stays masked (no timer/GIC on this core
    // yet), so WFE parks until the next event without a wake source — fine for a bring-up smoke test.
    loop {
        unsafe { core::arch::asm!("wfe", options(nomem, nostack, preserves_flags)) };
    }
}

/// BSP: release cores 1-3 from the spin-table. Write our entry point into each core's slot, clean
/// those writes to RAM (the parked cores read them MMU-off), then one `DSB` + broadcast `SEV`. `SEV`
/// wakes all parked cores; each re-checks its own slot and only the one now non-zero proceeds.
pub fn start_secondaries() {
    let entry = _secondary_start as usize as u64;
    unsafe {
        for core in 1..NUM_CORES {
            core::ptr::write_volatile(RELEASE_ADDR[core] as *mut u64, entry);
        }
        // All four slots (0xD8..0xF0) fall in one 64-byte cache line, so this single clean covers
        // cores 1-3; DSB completes it before the SEV makes the wake observable.
        let span = (RELEASE_ADDR[NUM_CORES - 1] + 8) - RELEASE_ADDR[1];
        cache::clean_range(RELEASE_ADDR[1], span);
        core::arch::asm!("dsb sy", "sev", options(nostack, preserves_flags));
    }
    serial_println!(":: AARCH64 SMP: released cores 1-3 via spin-table (0xE0/0xE8/0xF0) ::");
}
