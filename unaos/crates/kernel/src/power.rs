// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// ORIN-REBOOT — the arch-neutral POWER VERBS (baton orin-6 §5.1 + Peter's cold-boot ruling,
// 2026-08-25): `reboot` (warm reset) and `shutdown` (power OFF — the dark board is the
// "ready for cold boot" signal, and an idling board is wasted energy).
//
// ONE-OS law: each VERB is one word on every UnaOS, and this module is where they land; the
// MECHANISM behind each is per-platform, and every `cfg` below states its hardware reason.
// The contract of both verbs: emit the invocation witness, dispatch the platform mechanism,
// and never return — either the platform acts mid-instruction, or the honest failure witness
// prints and the core parks in `hlt_loop` (a machine that will not comply must say so, not
// pretend).
//
// Mechanisms:
//   * aarch64, non-Pi (Jetson Orin / QEMU `virt`): **PSCI via SMC** — invoke the firmware,
//     never build power logic ourselves. `SYSTEM_RESET` (0x8400_0009) for reboot,
//     `SYSTEM_OFF` (0x8400_0008) for shutdown. The Orin's ATF/BL31 monitor at EL3 services
//     the calls (the same conduit `smp_virt`'s `CPU_ON` uses); QEMU `virt`'s emulated PSCI
//     intercepts the SMC in TCG (reset restarts the machine, off exits it). SMCCC fast
//     calls (PSCI, Arm DEN0022); they return only on refusal.
//   * aarch64 + `pi` (Pi 4 bare-metal): NO PSCI — the Pi boots via armstub with no EL3
//     monitor, so an SMC would be an undefined-instruction trap, and the board's actual
//     reset/off paths (the BCM2711 PM/WDOG block, the mailbox) are the pi lane's to wire,
//     not this arc's. Honest witness + park.
//   * x86_64 reboot: the mechanism slot is deliberately UNWIRED this arc (the candidate
//     paths — FADT RESET_REG, the 8042 pulse — live in the x86 lane beside
//     `acpi_power::poweroff`, which is the rmbp seat's file). Honest witness + park.
//   * x86_64 shutdown: REAL — routed to the existing `crate::arch::acpi_power::poweroff()`
//     (ACPI S5, the crystal.rs Shut-Down path), which carries its own honest fallback.
//
// Witness families: `[orinreboot]` / `[orinshutoff]` (tokens > 8 bytes by construction —
// each bracket prefix alone is 12 bytes — so `strings` on the artifact finds them; the LLVM
// ≤8-byte immediate-encoding trap cannot swallow them).

/// Warm-reboot the machine via the platform's firmware mechanism. Never returns: either the
/// platform resets, or the failure witness prints and the core parks in `hlt_loop`.
pub fn reboot() -> ! {
    serial_println!("[orinreboot] reboot verb invoked — dispatching the platform mechanism");
    platform_reboot()
}

/// Power the machine OFF via the platform's firmware mechanism (cold-boot-ready: the next
/// boot is a cold one, and the dark board says so at a glance). Never returns: either the
/// platform cuts power, or the failure witness prints and the core parks in `hlt_loop`.
pub fn shutdown() -> ! {
    serial_println!("[orinshutoff] shutdown verb invoked — dispatching the platform mechanism");
    platform_shutdown()
}

// ── aarch64, non-Pi: PSCI via SMC ────────────────────────────────────────────────────────
// Hardware reason for the gate: PSCI is the Arm-standard firmware power interface and both
// boards on this path carry a monitor that serves it (Orin: ATF/BL31 at EL3; QEMU `virt`:
// emulated PSCI, `method = "smc"` — see `smp_virt::psci_call`'s conduit note). The Pi
// carries none.

/// PSCI (Arm DEN0022) `SYSTEM_RESET` — SMC32 function id. Architectural warm reset of the
/// whole system; on success the call does not return.
#[cfg(all(target_arch = "aarch64", not(feature = "pi")))]
const PSCI_SYSTEM_RESET: u64 = 0x8400_0009;
/// PSCI (Arm DEN0022) `SYSTEM_OFF` — SMC32 function id. Powers the system down; on success
/// the call does not return.
#[cfg(all(target_arch = "aarch64", not(feature = "pi")))]
const PSCI_SYSTEM_OFF: u64 = 0x8400_0008;

/// One SMCCC fast call via the SMC conduit (the `smpprobe`/`smp_virt` plumbing, restated
/// locally because both twins keep theirs private). x0-x17 are volatile per SMCCC; we
/// clobber x1-x17 and read x0. No `nomem`: a reset/off is a global side effect that must
/// not be reordered around the witness prints.
#[cfg(all(target_arch = "aarch64", not(feature = "pi")))]
fn psci_call(func: u64) -> i64 {
    let mut x0 = func;
    unsafe {
        core::arch::asm!(
            "smc #0",
            inout("x0") x0,
            out("x1") _, out("x2") _, out("x3") _,
            out("x4") _, out("x5") _, out("x6") _, out("x7") _,
            out("x8") _, out("x9") _, out("x10") _, out("x11") _,
            out("x12") _, out("x13") _, out("x14") _, out("x15") _,
            out("x16") _, out("x17") _,
            options(nostack),
        );
    }
    x0 as i64
}

#[cfg(all(target_arch = "aarch64", not(feature = "pi")))]
fn platform_reboot() -> ! {
    serial_println!(
        "[orinreboot] PSCI SYSTEM_RESET ({:#010x}) via SMC — firmware owns the machine from here",
        PSCI_SYSTEM_RESET
    );
    let ret = psci_call(PSCI_SYSTEM_RESET);
    // A returning SYSTEM_RESET is a refusal (NOT_SUPPORTED and friends are negative per PSCI).
    serial_println!(
        "[orinreboot] PSCI SYSTEM_RESET RETURNED ({}) — firmware refused the reset; parking in hlt",
        ret
    );
    crate::hlt_loop();
}

#[cfg(all(target_arch = "aarch64", not(feature = "pi")))]
fn platform_shutdown() -> ! {
    serial_println!(
        "[orinshutoff] PSCI SYSTEM_OFF ({:#010x}) via SMC — firmware owns the machine from here",
        PSCI_SYSTEM_OFF
    );
    let ret = psci_call(PSCI_SYSTEM_OFF);
    serial_println!(
        "[orinshutoff] PSCI SYSTEM_OFF RETURNED ({}) — firmware refused the off; parking in hlt",
        ret
    );
    crate::hlt_loop();
}

// ── Pi 4 bare-metal: no EL3 monitor, no PSCI ─────────────────────────────────────────────
// An SMC here traps, and the BCM2711 PM/WDOG + mailbox power paths are the pi lane's to
// wire. Refuse honestly.

#[cfg(all(target_arch = "aarch64", feature = "pi"))]
fn platform_reboot() -> ! {
    serial_println!(
        "[orinreboot] no reboot mechanism wired on this platform (Pi: no PSCI; the BCM2711 PM/WDOG path is the pi lane's) — parking in hlt"
    );
    crate::hlt_loop();
}

#[cfg(all(target_arch = "aarch64", feature = "pi"))]
fn platform_shutdown() -> ! {
    serial_println!(
        "[orinshutoff] no shutdown mechanism wired on this platform (Pi: no PSCI; the mailbox power path is the pi lane's) — parking in hlt"
    );
    crate::hlt_loop();
}

// ── x86_64 ───────────────────────────────────────────────────────────────────────────────

/// x86_64 reboot: the mechanism slot is unwired this arc — the candidate paths (FADT
/// RESET_REG / 8042 pulse) belong beside `acpi_power::poweroff` in the x86 lane. Refuse
/// honestly, the same shape as the S5 fallback.
#[cfg(target_arch = "x86_64")]
fn platform_reboot() -> ! {
    serial_println!(
        "[orinreboot] no reboot mechanism wired on this platform yet (x86: FADT RESET_REG slot is the rmbp lane's) — parking in hlt"
    );
    crate::hlt_loop();
}

/// x86_64 shutdown: REAL — ACPI S5 through the existing `acpi_power::poweroff` (the
/// crystal.rs Shut-Down path), which discovers `\_S5_` honestly and parks in `hlt` with its
/// own witness if any required fact is missing.
#[cfg(target_arch = "x86_64")]
fn platform_shutdown() -> ! {
    serial_println!("[orinshutoff] x86 mechanism: ACPI S5 (acpi_power::poweroff)");
    crate::arch::acpi_power::poweroff();
}
