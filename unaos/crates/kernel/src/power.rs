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
// PWRDRAIN (SO31, trunk queue §1 (f)): EVERY verb below flushes the serial staging ring — uncapped,
// through the arch's raw lock-free writer — immediately before it hands the machine to the firmware,
// and emits `[<family>] ring drained lines=N bytes=M`. The reason is the one property the transport
// cannot give here: a contended line is DEFERRED, and a deferred line is safe only while a next print
// exists. A power verb is the context where there is none, so the ring dies with the power and the
// verb's own announce can die with it.
//
// RBTDRAIN (rmbp-ledger A3, same doc section): PWRDRAIN is only the FIRST of TWO buffers on the 2012
// rMBP. `power_drain` ends in `serial::_print`, whose x86 sinks are the 16550 at 0x3F8 — which this
// laptop does not have — and the FTDI MIRROR RING, and that ring reaches the cable only when the xHCI
// device-service pass runs. A power verb is a context with no next pass, so on the rMBP the staged
// lines arrived one buffer short of a human and the whole reboot ladder died there. Every x86 verb
// below therefore follows its `power_drain` with [`ftdi_flush_witness`] — a bounded, non-blocking,
// synchronous pump of that ring — and says on the wire how much left and whether any was left behind.
// See `serial_ring::power_drain` and
// `docs/dev/OS/02_KERNEL_CORE/serial_transport.md` §"The power verbs drain first".
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
    // PWRDRAIN (SO31, trunk queue §1 (f)): the announce above may have been DEFERRED into the serial
    // staging ring by a contended lock, and the reset takes the machine microseconds from here — a
    // deferred line is not a lost one only while a next print exists, and past this call there is
    // none. Flush the whole ring, uncapped, through the raw lock-free writer, then say so.
    crate::serial_ring::power_drain("pwrreboot");
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
    // PWRDRAIN (SO31, trunk queue §1 (f)): render13 boot 1 was dropping 5 331 lines into a saturated
    // ring; on that boot every line still staged when SYSTEM_OFF landed died with the power, this
    // verb's own witness included. Flush the ring whole, uncapped, before the SMC.
    crate::serial_ring::power_drain("pwrshutoff");
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
    // PWRDRAIN: `hlt_loop` is also a context with no next print — a staged line would sit in the ring
    // until something else printed, and nothing will. Flush it here for the same reason the SMC paths
    // do, and the witness says the refusal really reached the wire.
    crate::serial_ring::power_drain("pwrreboot");
    crate::hlt_loop();
}

#[cfg(all(target_arch = "aarch64", feature = "pi"))]
fn platform_shutdown() -> ! {
    serial_println!(
        "[pwrshutoff] no shutdown mechanism wired on this platform (Pi: no PSCI; the mailbox power path is the pi lane's) — parking in hlt"
    );
    // PWRDRAIN: same as the reboot twin above — a park is a context with no next print.
    crate::serial_ring::power_drain("pwrshutoff");
    crate::hlt_loop();
}

// ── x86_64 ───────────────────────────────────────────────────────────────────────────────

/// x86_64 reboot: REAL — the FADT RESET_REG / 8042 ladder in `acpi_power::reboot` (beside the
/// S5 `poweroff`, the same shape: discover honestly, witness before every write, park in `hlt`
/// with its own line if the platform will not comply). It takes no lock past this line.
#[cfg(target_arch = "x86_64")]
fn platform_reboot() -> ! {
    serial_println!("[pwrreboot] x86 mechanism: FADT RESET_REG ladder (acpi_power::reboot)");
    // PWRDRAIN: `acpi_power::reboot` already drains (it has since LOCKFIX), but it does so with no
    // witness, so a capture could not tell a flushed ring from a ring that was never reached. The
    // count goes on the wire here; the second drain downstream then finds the ring empty.
    crate::serial_ring::power_drain("pwrreboot");
    // RBTDRAIN (rmbp-ledger A3): the drain above ends in `serial::_print`, and on THIS laptop
    // `_print`'s x86 sinks are a 16550 that does not exist and the FTDI MIRROR RING. So everything
    // flushed a line ago is now one buffer from the cable and no closer to a human: the ring reaches
    // the wire only when the xHCI device-service pass runs, and past this point there is no next
    // pass. Push it out synchronously, here, while interrupts are still on and the event ring can
    // still be pumped (`acpi_power::reboot` masks them as its second statement, and a masked
    // `hlt()` inside the TX pump would never wake).
    ftdi_flush_witness("pwrreboot");
    crate::arch::acpi_power::reboot();
}

/// RBTDRAIN — flush the FTDI mirror ring and put the result on the wire, for one power verb.
///
/// ### The witness is printed AFTER the flush it reports and BEFORE the one that carries it
///
/// The obvious placement — print, then flush — cannot work: `bytes=`, `transfers=` and `exhausted=`
/// do not exist until the pump has run, and a witness that reported a flush it had not yet performed
/// would be exactly the lie this arc exists to remove. The obvious alternative — flush, then print —
/// leaves the line itself unflushed, sitting in the mirror ring when the reset lands.
///
/// So the two power verbs and their platform PORTS compose, and the composition is the mechanism:
/// this call flushes the verb's announce and `power_drain`'s tally, then prints its own line into the
/// ring; `acpi_power::reboot`'s FIRST STATEMENT is the same flush again, and THAT is what carries
/// this line to the cable. The port's copy is the one no caller can skip (S5DRAIN put
/// `s5_ring_flush` at `poweroff()`'s port for that reason and this follows it); this copy is the one
/// that reports, in the verb's own announce order. Neither is redundant, and the second finds only
/// what the first wrote.
///
/// `hw_wait_budget()` is the span, the same one every bounded hardware wait in the tree uses: ~1.1 s
/// on the rMBP's 2.3 GHz Ivy Bridge, ~2.5 s under TCG. The ring at a reboot holds the tail of a log
/// the service pass has been draining all along — hundreds of bytes, microseconds of cable — so the
/// budget binds only when something is already wrong, which is the case it exists for.
#[cfg(target_arch = "x86_64")]
fn ftdi_flush_witness(tag: &str) {
    let (bytes, transfers, exhausted) =
        crate::drivers::xhci::ftdi_flush_sync(crate::arch::hw_wait_budget());
    serial_println!(
        "[{}] ftdi flushed bytes={} transfers={} exhausted={}",
        tag, bytes, transfers, if exhausted { 1 } else { 0 }
    );
}

/// x86_64 shutdown: REAL — ACPI S5 through the existing `acpi_power::poweroff` (the
/// crystal.rs Shut-Down path), which discovers `\_S5_` honestly and parks in `hlt` with its
/// own witness if any required fact is missing.
#[cfg(target_arch = "x86_64")]
fn platform_shutdown() -> ! {
    serial_println!("[pwrshutoff] x86 mechanism: ACPI S5 (acpi_power::poweroff)");
    // PWRDRAIN: x86's S5 path used not to share the reboot ladder's drain — `acpi_power::poweroff`
    // masks interrupts and writes PM1_CNT with whatever is still staged. S5DRAIN (trunk queue §5,
    // 2026-09-12) closes the ⚠ SCOPE box that stood here: the flush is now the FIRST statement of
    // `poweroff()` itself, so `video/crystal.rs`'s Shut Down and `video/instgui.rs`, which call it
    // DIRECTLY, drain too. This call stays and is not redundant — it puts the count on the wire in
    // THIS verb's announce order, and the flush at the port then finds the ring empty (`lines=0`).
    crate::serial_ring::power_drain("pwrshutoff");
    // RBTDRAIN: the same last leg as the reboot twin above, and needed for the same reason — the
    // staging drain hands the lines to the FTDI mirror ring, and S5 kills the machine before the
    // device-service pass can carry them out the cable. So this call is what puts the two announces
    // above and `ring drained lines=N bytes=M` ON THE CABLE, which on this laptop is the whole of
    // what an operator sees.
    //
    // ⚠ WHAT IT DOES NOT DO, said plainly rather than left to be discovered: the `[pwrshutoff] ftdi
    // flushed …` line it prints goes INTO the ring like any other, and on the S5 route nothing takes
    // it back out — `poweroff()`'s port flush is `s5_ring_flush`, the STAGING ring, and there is no
    // FTDI flush at that port. On the reboot route `acpi_power::reboot`'s first statement is exactly
    // that flush and so carries its twin. So this tally is a serial-log fact and not yet a cable
    // fact, and the same gap covers `video/crystal.rs`'s Shut Down and `video/instgui.rs`, which
    // call `acpi_power::poweroff` DIRECTLY and reach S5 with the MIRROR ring unflushed altogether
    // (their STAGING ring is flushed — that is what S5DRAIN fixed). One statement folded onto
    // `poweroff()`'s signature, exactly as this arc folded one onto `reboot()`'s, closes all three at
    // the port. It is outside RBTDRAIN's brief; it is REPORTED as a STOP, not taken.
    ftdi_flush_witness("pwrshutoff");
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
    // PWRDRAIN: the announce above is the operator's evidence that the menu verb acted, and it is the
    // line most likely to be staged — a desktop press happens with the compositor printing. Flush
    // before the SMC. The witness rides the `[pwrreboot]` family, not `[crystal]`: it is a statement
    // about the TRANSPORT, and a reader grepping `ring drained` wants both verbs in one tally.
    crate::serial_ring::power_drain("pwrreboot");
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
    // PWRDRAIN: trunk queue §1 (f) names this call site by name. Flush the ring whole before the SMC.
    crate::serial_ring::power_drain("pwrshutoff");
    let ret = psci_call(PSCI_SYSTEM_OFF);
    serial_println!(
        "[crystal] verb=shutdown -> PSCI SYSTEM_OFF RETURNED ret={} — a returning PSCI power call is a REFUSAL; parking in hlt",
        ret
    );
    crate::hlt_loop();
}
