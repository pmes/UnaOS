// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// ORIN-REBOOT (watchdog half) — the `UNAOS_ORINWDT=1` BOOT WATCHDOG on the Tegra234 TKE
// (Timer Kernel Engine), so a wedged Orin boot SELF-RESETS without bench hands (there is
// deliberately no bench power-switch hardware; today a dark boot means a human pulling the
// barrel jack).
//
// Semantics (boot-scoped, matching the baton's "a wedged boot self-resets"):
//   * `boot_arm()` — fired early in `tegra_early_stop`, right after the MMU + exception
//     vectors are live: program WDT0 for a **system POR reset on the 5th expiration**,
//     `TIMEOUT_SECS` in total, and start it. Nothing pets it.
//   * `boot_ok_disarm()` — fired on the EL1 terminus line (the boot reached the scheduler):
//     unlock + stop the counter. A boot that wedges anywhere between the two calls power-
//     cycles itself when the window runs out; a boot that completes leaves no watchdog
//     running (the shell/CAPSTONE regime has no petting service, so leaving it armed would
//     reset a HEALTHY machine — the disarm is the correctness half of the knob).
//
// Documentation of record: Linux `drivers/clocksource/timer-tegra186.c` (the
// `nvidia,tegra234-timer` driver — the same role pcie-tegra194 played for NET-3), register
// names kept 1:1 so the two read side-by-side. Layout facts from it + `tegra234.dtsi`:
//   * TKE at 0x0208_0000 (`timer@2080000`), one 0x1_0000 block per sub-unit:
//     TKE shared regs at +0, TMR0..15 at +0x1_0000*(1+i), WDT0..1 at +0x1_0000*(17+i)
//     (tegra234 soc table: num_timers=16, num_wdts=2) — WDT0 = 0x0219_0000.
//   * A WDT counts EXPIRATIONS of a TKE TMR it points at (`WDTCR.TIMER_SOURCE`); the 5th
//     expiration fires the enabled POR reset. So total timeout = 5 * TMR period, and the
//     TMR runs PERIODIC off the µs clock (`TMRCSSR_SRC_USEC`).
//   * `WDTUR` must see the unlock pattern 0x0000_C45A before `WDTCMDR.DISABLE_COUNTER`.
//   * A WDTCR carrying `LOCAL_INT_ENABLE` means firmware configured (and possibly
//     write-locked) the block — mirror the driver: inherit that config, still start/stop
//     the counter, and say so in the witness.
//
// MMIO: raw physical pointers, `read/write_volatile` — the TKE lives in GiB 0 of the Tegra
// Device-nGnRE window `mmu_tegra::init` maps into BOTH translation tables (the same window
// the 16550 at 0x0C28_0000 writes through), so both the EL2 arm site and the post-drop EL1
// disarm site reach it identity-mapped.
//
// QEMU: `tegra`-gated module — the virt gate never compiles it, and QEMU models no TKE.
// Witness family: `[orinreboot]` (>8-byte tokens; artifact-grep-able by `strings`).

/// TKE base — `timer@2080000` in tegra234.dtsi (fixed silicon address, same class as the
/// 16550 base in `serial.rs`).
const TKE_BASE: u64 = 0x0208_0000;
/// One 0x1_0000 block per TKE sub-unit (timer-tegra186.c `tegra186_tmr_create` /
/// `tegra186_wdt_create` offset math).
const TKE_STRIDE: u64 = 0x1_0000;
/// Tegra234 TKE geometry (timer-tegra186.c `tegra234_timer` soc table).
const NUM_TMRS: u64 = 16;
/// WDT0 register block: TKE + the 16 TMR blocks.
const WDT0_BASE: u64 = TKE_BASE + TKE_STRIDE * (1 + NUM_TMRS); // = 0x0219_0000

// --- watchdog registers (offsets from WDT0_BASE; names 1:1 with timer-tegra186.c) ---
const WDTCR: u64 = 0x000;
const WDTCR_SYSTEM_POR_RESET_ENABLE: u32 = 1 << 16;
const WDTCR_LOCAL_INT_ENABLE: u32 = 1 << 12;
const WDTCR_PERIOD_MASK: u32 = 0xff << 4;
const WDTCR_PERIOD_1: u32 = 1 << 4;
const WDTCR_TIMER_SOURCE_MASK: u32 = 0xf;
const WDTSR: u64 = 0x004;
const WDTCMDR: u64 = 0x008;
const WDTCMDR_DISABLE_COUNTER: u32 = 1 << 1;
const WDTCMDR_START_COUNTER: u32 = 1 << 0;
const WDTUR: u64 = 0x00c;
const WDTUR_UNLOCK_PATTERN: u32 = 0x0000_c45a;

// --- timer registers (offsets from the source TMR's block) ---
const TMRCR: u64 = 0x000;
const TMRCR_ENABLE: u32 = 1 << 31;
const TMRCR_PERIODIC: u32 = 1 << 30;
const TMRSR: u64 = 0x004;
const TMRSR_INTR_CLR: u32 = 1 << 30;
const TMRCSSR: u64 = 0x008;
const TMRCSSR_SRC_USEC: u32 = 0;

/// Total boot window in seconds: POR reset fires this long after `boot_arm` unless
/// `boot_ok_disarm` runs first. 5 minutes — generous against the slowest attended boots
/// (USB settle, panel bring-up) while still turning a dark bench around unattended. The TMR
/// period is a fifth of this (60 s = 60_000_000 µs, well under the 28-bit PTV ceiling).
const TIMEOUT_SECS: u32 = 300;

#[inline]
fn tmr_base(index: u32) -> u64 {
    TKE_BASE + TKE_STRIDE * (1 + index as u64)
}

#[inline]
fn mmio_read(base: u64, off: u64) -> u32 {
    unsafe { core::ptr::read_volatile((base + off) as *const u32) }
}

#[inline]
fn mmio_write(base: u64, off: u64, value: u32) {
    unsafe { core::ptr::write_volatile((base + off) as *mut u32, value) }
}

/// Arm WDT0 for a system POR reset `TIMEOUT_SECS` from now. Call once, early on the tegra
/// boot path, with the MMU (Device window) live and serial up.
pub fn boot_arm() {
    // Read the block's live config first: the timer source the firmware left selected is the
    // one we drive (mirroring the driver, which trusts WDTCR.TIMER_SOURCE over any DT hint),
    // and a pre-set LOCAL_INT_ENABLE marks the config firmware-owned/locked.
    let cr = mmio_read(WDT0_BASE, WDTCR);
    let locked = cr & WDTCR_LOCAL_INT_ENABLE != 0;
    let source = cr & WDTCR_TIMER_SOURCE_MASK;
    let tmr = tmr_base(source);

    // Quiesce: unlock + stop the counter, stop the source timer. Safe on an idle block (the
    // disable of a stopped counter is a no-op) and required on an armed one.
    mmio_write(WDT0_BASE, WDTUR, WDTUR_UNLOCK_PATTERN);
    mmio_write(WDT0_BASE, WDTCMDR, WDTCMDR_DISABLE_COUNTER);
    mmio_write(tmr, TMRCR, 0);

    // Source timer: clear any latched interrupt, clock it off the µs source, run PERIODIC at
    // a fifth of the total window (the reset is the 5th expiration).
    let period_us = (TIMEOUT_SECS / 5) * 1_000_000; // 28-bit PTV field; 60e6 fits
    mmio_write(tmr, TMRSR, TMRSR_INTR_CLR);
    mmio_write(tmr, TMRCSSR, TMRCSSR_SRC_USEC);
    mmio_write(tmr, TMRCR, TMRCR_ENABLE | TMRCR_PERIODIC | period_us);

    // Watchdog config — only when the firmware has not locked it: keep the selected source,
    // one timer period per expiration, POR reset enabled. No LOCAL_INT (this kernel wires no
    // TKE IRQ handler; the watchdog's ONLY job here is the reset).
    if !locked {
        let new_cr = (cr & !(WDTCR_PERIOD_MASK) & !WDTCR_TIMER_SOURCE_MASK)
            | source
            | WDTCR_PERIOD_1
            | WDTCR_SYSTEM_POR_RESET_ENABLE;
        mmio_write(WDT0_BASE, WDTCR, new_cr);
    }

    mmio_write(WDT0_BASE, WDTCMDR, WDTCMDR_START_COUNTER);

    // Witness with READ-BACKS, not intentions: the CR the block now holds and its status
    // register. `locked=1` says the config half was inherited from firmware, not written.
    serial_println!(
        "[orinreboot] wdt ARMED — POR reset in {}s unless the boot completes (WDT0@{:#x} tmr{} period {}s x5, WDTCR={:#010x} WDTSR={:#010x} locked={})",
        TIMEOUT_SECS,
        WDT0_BASE,
        source,
        TIMEOUT_SECS / 5,
        mmio_read(WDT0_BASE, WDTCR),
        mmio_read(WDT0_BASE, WDTSR),
        locked as u8,
    );
}

/// The boot reached the EL1 terminus (scheduler handoff): unlock + stop the counter and its
/// source timer, so the running system is not reset by its own boot insurance.
pub fn boot_ok_disarm() {
    let source = mmio_read(WDT0_BASE, WDTCR) & WDTCR_TIMER_SOURCE_MASK;
    mmio_write(WDT0_BASE, WDTUR, WDTUR_UNLOCK_PATTERN);
    mmio_write(WDT0_BASE, WDTCMDR, WDTCMDR_DISABLE_COUNTER);
    mmio_write(tmr_base(source), TMRCR, 0);
    serial_println!(
        "[orinreboot] wdt DISARMED — boot reached the EL1 terminus (WDTSR={:#010x})",
        mmio_read(WDT0_BASE, WDTSR),
    );
}
