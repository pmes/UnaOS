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
//   * x86_64 reboot: REAL (FADTRESET) — routed to `crate::arch::acpi_power::reboot()`, the
//     ladder beside `poweroff` in the rmbp seat's file: the FADT RESET_REG (ACPI §4.8.3.6,
//     honoured in SystemIO or SystemMemory space, only from a checksummed FADT that flags
//     RESET_REG_SUP), then the 8042 pulse (0xFE to port 0x64), then the honest park. Each rung
//     prints its own witness before it acts, lock-free (LOCKFIX).
//   * x86_64 shutdown: REAL — routed to the existing `crate::arch::acpi_power::poweroff()`
//     (ACPI S5, the crystal.rs Shut-Down path), which carries its own honest fallback.
//
// Witness families: `[pwrreboot]` / `[pwrshutoff]` (tokens > 8 bytes by construction —
// each bracket prefix alone is 11+ bytes — so `strings` on the artifact finds them; the LLVM
// ≤8-byte immediate-encoding trap cannot swallow them).

/// Warm-reboot the machine via the platform's firmware mechanism. Never returns: either the
/// platform resets, or the failure witness prints and the core parks in `hlt_loop`.
pub fn reboot() -> ! {
    serial_println!("[pwrreboot] reboot verb invoked — dispatching the platform mechanism");
    platform_reboot()
}

/// Power the machine OFF via the platform's firmware mechanism (cold-boot-ready: the next
/// boot is a cold one, and the dark board says so at a glance). Never returns: either the
/// platform cuts power, or the failure witness prints and the core parks in `hlt_loop`.
pub fn shutdown() -> ! {
    serial_println!("[pwrshutoff] shutdown verb invoked — dispatching the platform mechanism");
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
    let mut x0 = func; #[cfg(feature = "ga10bprobe4d")] if func == PSCI_SYSTEM_OFF { crate::arch::ga10b_probe::ga10bprobe4_deferred_run(); } // GA10B-PROBE4D (orin 27, 2026-09-12; Peter: the desktop first, the GPU probe LAST): under `ga10bprobe4d` (UNAOS_GA10B_PROBE4=4) EVERY PSCI SYSTEM_OFF request — shell `shutdown`/`off` via `shutdown()`, crystal Shut Down via `crystal_shutdown()`, a probe's own finish — passes through this ONE call before the SMC; it is a no-op unless the rung armed itself at boot, and it CONSUMES the arm on entry so the rung's own finish4 -> shutdown re-entry falls straight through to the OFF. SYSTEM_RESET does not trigger it (after 4b's lock the next boot must be cold). Appended to THIS line for knob-off byte identity: cfg-erased, no `Location` below moves.
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
        "[pwrreboot] PSCI SYSTEM_RESET ({:#010x}) via SMC — firmware owns the machine from here",
        PSCI_SYSTEM_RESET
    );
    let ret = psci_call(PSCI_SYSTEM_RESET);
    // A returning SYSTEM_RESET is a refusal (NOT_SUPPORTED and friends are negative per PSCI).
    serial_println!(
        "[pwrreboot] PSCI SYSTEM_RESET RETURNED ({}) — firmware refused the reset; parking in hlt",
        ret
    );
    crate::hlt_loop();
}

#[cfg(all(target_arch = "aarch64", not(feature = "pi")))]
fn platform_shutdown() -> ! {
    serial_println!(
        "[pwrshutoff] PSCI SYSTEM_OFF ({:#010x}) via SMC — firmware owns the machine from here",
        PSCI_SYSTEM_OFF
    );
    let ret = psci_call(PSCI_SYSTEM_OFF);
    serial_println!(
        "[pwrshutoff] PSCI SYSTEM_OFF RETURNED ({}) — firmware refused the off; parking in hlt",
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
        "[pwrreboot] no reboot mechanism wired on this platform (Pi: no PSCI; the BCM2711 PM/WDOG path is the pi lane's) — parking in hlt"
    );
    crate::hlt_loop();
}

#[cfg(all(target_arch = "aarch64", feature = "pi"))]
fn platform_shutdown() -> ! {
    serial_println!(
        "[pwrshutoff] no shutdown mechanism wired on this platform (Pi: no PSCI; the mailbox power path is the pi lane's) — parking in hlt"
    );
    crate::hlt_loop();
}

// ── x86_64 ───────────────────────────────────────────────────────────────────────────────

/// x86_64 reboot: REAL — the FADT RESET_REG / 8042 ladder in `acpi_power::reboot` (beside the
/// S5 `poweroff`, the same shape: discover honestly, witness before every write, park in `hlt`
/// with its own line if the platform will not comply). It takes no lock past this line.
#[cfg(target_arch = "x86_64")]
fn platform_reboot() -> ! {
    serial_println!("[pwrreboot] x86 mechanism: FADT RESET_REG ladder (acpi_power::reboot)");
    crate::arch::acpi_power::reboot();
}

/// x86_64 shutdown: REAL — ACPI S5 through the existing `acpi_power::poweroff` (the
/// crystal.rs Shut-Down path), which discovers `\_S5_` honestly and parks in `hlt` with its
/// own witness if any required fact is missing.
#[cfg(target_arch = "x86_64")]
fn platform_shutdown() -> ! {
    serial_println!("[pwrshutoff] x86 mechanism: ACPI S5 (acpi_power::poweroff)");
    crate::arch::acpi_power::poweroff();
}

// ── CRYSTAL (A34) — the power verbs as the DESKTOP invokes them ───────────────────────────
//
// Peter, render7 2026-09-06: *"crystal restart/shut down not working"*. Both verbs reached their
// terminus on the wire (`:: SHARD-MENU: crystal_pick verb=ShutDown action=real ::`) and neither
// acted — `video/crystal.rs` printed a Pi-era `unimplemented:` line on EVERY aarch64 board,
// including the Orin, whose ATF/BL31 monitor at EL3 answers PSCI on the very SMC conduit
// `smp_virt`'s `CPU_ON` already uses and proves every boot. The verbs were never missing a
// MECHANISM — this module has carried both since ORIN-REBOOT. They were missing the CALL.
//
// Why these two entries exist instead of the menu calling `reboot()`/`shutdown()` directly: the
// witness a flight scores for A34 is a `[crystal]` line, and it must carry the SMC return value,
// because **a PSCI power call that RETURNS has failed**. An operator who pressed Shut Down is owed
// that fact in the family they are grepping, not one line down in another. The four-line mechanism
// body below is duplicated from `platform_reboot`/`platform_shutdown` deliberately: the alternative
// is turning those `-> !` functions fallible, which rewrites lines in the middle of a file three
// other callers (`shell`, `ga10b_probe`, `selfup_tegra`) already depend on.
//
// GATED `not(feature = "pi")`, and that gate is the honest half of A34. The Pi 4 boots bare-metal to
// EL2 with no EL3 monitor behind the `smc`, so there is nothing to call; a menu verb that PARKED the
// Pi's desktop in `hlt` to look decisive would be strictly worse than the `unimplemented:` line it
// prints today. On the Pi these symbols do not exist, `crystal.rs` keeps its unchanged arm, and the
// Pi's kernel8 image is unmoved by this half of the arc.

/// CRYSTAL **Restart** — announce on the `[crystal]` family, then PSCI `SYSTEM_RESET` through the
/// SMC. Never returns: either the firmware resets the board mid-instruction, or the refusal witness
/// names the return code and the core parks in `hlt_loop`.
#[cfg(all(target_arch = "aarch64", not(feature = "pi")))]
pub fn crystal_restart() -> ! {
    serial_println!("[crystal] verb=restart -> PSCI SYSTEM_RESET");
    let ret = psci_call(PSCI_SYSTEM_RESET);
    serial_println!(
        "[crystal] verb=restart -> PSCI SYSTEM_RESET RETURNED ret={} — a returning PSCI power call is a REFUSAL; parking in hlt",
        ret
    );
    crate::hlt_loop();
}

/// CRYSTAL **Shut Down** — announce on the `[crystal]` family, then PSCI `SYSTEM_OFF` through the
/// SMC. Same contract as [`crystal_restart`]: the announce precedes the action, and the only line
/// that can follow it is the refusal.
#[cfg(all(target_arch = "aarch64", not(feature = "pi")))]
pub fn crystal_shutdown() -> ! {
    serial_println!("[crystal] verb=shutdown -> PSCI SYSTEM_OFF");
    let ret = psci_call(PSCI_SYSTEM_OFF);
    serial_println!(
        "[crystal] verb=shutdown -> PSCI SYSTEM_OFF RETURNED ret={} — a returning PSCI power call is a REFUSAL; parking in hlt",
        ret
    );
    crate::hlt_loop();
}
