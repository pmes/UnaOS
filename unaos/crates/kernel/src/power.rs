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
// rMBP. `power_drain` writes `_print`'s SINK SET via `sink_write` (SINKDRAIN): the 16550 at 0x3F8 — which this
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
    crate::bootlog_println!("[pwrreboot] reboot verb invoked — dispatching the platform mechanism");
    prelude("reboot"); #[cfg(feature = "selfdiag")] crate::bootwit::flush("reboot"); // POWERMENU M1. SELFDIAG M1 (B324): the boot log reaches the disk before the reset
    crate::fs::fat::flush_pending_dirent_by("shutdown"); // SDHCMULTI2 M2: a deferred dir entry must not die with the power
    platform_reboot()
}

/// Power the machine OFF via the platform's firmware mechanism (cold-boot-ready: the next
/// boot is a cold one, and the dark board says so at a glance). Never returns: either the
/// platform cuts power, or the failure witness prints and the core parks in `hlt_loop`.
pub fn shutdown() -> ! {
    serial_println!("[pwrshutoff] shutdown verb invoked — dispatching the platform mechanism");
    prelude("shutdown"); #[cfg(feature = "selfdiag")] crate::bootwit::flush("shutdown"); // POWERMENU M1. SELFDIAG M1 (B324): the boot log reaches the disk before S5
    #[cfg(all(target_arch = "x86_64", feature = "nvidia-kepler-vblank"))]
    crate::drivers::gpu::kepler_vblank::shutdown_census(); // KVBLANK6 — the kept-live vector's last census
    crate::fs::fat::flush_pending_dirent_by("shutdown"); // SDHCMULTI2 M2
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
    crate::bootlog_println!(
        "[pwrreboot] PSCI SYSTEM_RESET ({:#010x}) via SMC — firmware owns the machine from here",
        PSCI_SYSTEM_RESET
    );
    // PWRDRAIN (SO31, trunk queue §1 (f)): the announce above may have been DEFERRED into the serial
    // staging ring by a contended lock, and the reset takes the machine microseconds from here — a
    // deferred line is not a lost one only while a next print exists, and past this call there is
    // none. Flush the whole ring, uncapped, through the raw lock-free writer, then say so.
    going("reboot"); // POWERMENU M1
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
    going("shutdown"); // POWERMENU M1
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
    crate::bootlog_println!(
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
    crate::bootlog_println!("[pwrreboot] x86 mechanism: FADT RESET_REG ladder (acpi_power::reboot)");
    // PWRDRAIN: `acpi_power::reboot` already drains (it has since LOCKFIX), but it does so with no
    // witness, so a capture could not tell a flushed ring from a ring that was never reached. The
    // count goes on the wire here; the second drain downstream then finds the ring empty.
    crate::serial_ring::power_drain("pwrreboot");
    // RBTDRAIN (rmbp-ledger A3): the drain above writes `_print`'s sink set via `sink_write` (SINKDRAIN), not `_print` itself, and on THIS laptop
    // `_print`'s x86 sinks are a 16550 that does not exist and the FTDI MIRROR RING. So everything
    // flushed a line ago is now one buffer from the cable and no closer to a human: the ring reaches
    // the wire only when the xHCI device-service pass runs, and past this point there is no next
    // pass. Push it out synchronously, here, while interrupts are still on and the event ring can
    // still be pumped (`acpi_power::reboot` masks them as its second statement, and a masked
    // `hlt()` inside the TX pump would never wake).
    ftdi_flush_witness("pwrreboot");
    going("reboot"); // POWERMENU M1 — the last witness before the firmware call
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
    // The `[pwrshutoff] ftdi flushed …` line this prints goes INTO the ring like any other, and what
    // takes it back out is `acpi_power::poweroff`'s own FTDI flush, folded onto that function's
    // signature line beside `s5_ring_flush` — the S5 twin of the fold on `acpi_power::reboot`. The
    // two compose exactly as the reboot pair does: this call reports in the verb's announce order,
    // the PORT's call is the one no caller can skip, and it is what carries this tally to the cable.
    // That fold is also why `video/crystal.rs`'s Shut Down and `video/instgui.rs` — which call
    // `poweroff` DIRECTLY, bypassing this function entirely — now reach the cable at all.
    ftdi_flush_witness("pwrshutoff");
    going("shutdown"); // POWERMENU M1 — the last witness before the firmware call
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
    going("reboot"); // POWERMENU M1
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
    going("shutdown"); // POWERMENU M1
    crate::serial_ring::power_drain("pwrshutoff");
    let ret = psci_call(PSCI_SYSTEM_OFF);
    serial_println!(
        "[crystal] verb=shutdown -> PSCI SYSTEM_OFF RETURNED ret={} — a returning PSCI power call is a REFUSAL; parking in hlt",
        ret
    );
    crate::hlt_loop();
}

// ── POWERMENU (R75) M1 — the CLEAN prelude every power verb runs before the firmware call ─────────
//
// Order: flush the deferred FAT dirent (SDHCMULTI2) -> close every window (LOGOUTDESK's sweep: the apps' own
// teardown paths run while the machine is still alive) -> stop the HDA output streams (no pop / no DMA into a
// powering-down chipset) -> [`going`] prints the one witness line immediately before the ACPI/PSCI call, so it is the
// LAST `:: POWER:` line and the firmware-side drain lines follow it.
static PRE_WIN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static PRE_FLUSH: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static PRE_HDA: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Run the clean prelude for `action` (`"shutdown"` / `"reboot"`). Idempotent enough: a second call finds nothing to do.
pub fn prelude(action: &str) {
    use core::sync::atomic::Ordering;
    let _ = action; #[cfg(all(target_arch = "x86_64", feature = "installdemo"))] crate::install::abimage::note_user_shutdown(action); // ABIMAGE (B683): the user's Restart / Shut Down (shell verbs, crystal menu) arms a staged image — `SHUTDOWN.REC why=user`, before the dirent flush below. LINE-NEUTRAL append, code first
    let flushed = crate::fs::fat::dirent_pending();
    crate::fs::fat::flush_pending_dirent_by("shutdown");
    PRE_FLUSH.store(flushed, Ordering::Relaxed);
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    { let (n, _k) = crate::video::wm::close_all_furniture(); PRE_WIN.store(n as u32, Ordering::Relaxed); }
    #[cfg(all(target_arch = "x86_64", feature = "hda"))]
    PRE_HDA.store(crate::drivers::hda::powerdown::stop(), Ordering::Relaxed);
}

/// The witness: printed once, right before the firmware call.
pub fn going(action: &str) {
    use core::sync::atomic::Ordering;
    serial_println!(":: POWER: action={} windows_closed={} flushed={} hda_stopped={} -> going ::", action,
        PRE_WIN.load(Ordering::Relaxed), PRE_FLUSH.load(Ordering::Relaxed) as u8, PRE_HDA.load(Ordering::Relaxed) as u8);
}

// LOGINWINDOW (B430) — the SLEEP lane's two entry points, declared as STUBS until LIDSLEEP (B431) lands: LIDSLEEP's
// fold REPLACES both bodies (the login window's Sleep button and its greying call only these). `unarmed` = no sleep
// ladder in this tree; the caller greys the button and says so on the wire.
/// Sleep the machine (LIDSLEEP's ladder). Returns the lane's word: `unarmed` until LIDSLEEP's fold.
pub fn sleep_request() -> &'static str {
    // SLEEP (B691): the sleep STATE (`idle`: backlight off, panel held, cores in hlt; S3 unreachable, `power::sleep` says why).
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    { return crate::video::lidsleep::sleep_request("loginwindow").word(); }
    #[allow(unreachable_code)]
    "unarmed"
}

/// `true` when [`sleep_request`] would act (the login window's Sleep button is live, not greyed).
pub fn sleep_armed() -> bool {
    cfg!(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))) // LIDSLEEP (B431): the lane blanks the backlight today; the lid ladder itself arms after rung 0 flies
}

// ── SLEEP (rmbp-ledger B691, BP-bar-20, docs/dev/blueprint/power.md) — the sleep state's record and its line ──────
//
// FILE-TAIL append under `lidsleep` + `wc` (x86; the panel half is a `wc` module), so no line above moves and a knob-off build carries none of it.
// The state is `idle`, not `s3`: ACPI S3 is unreachable from this tree (no AML interpreter for `_PTS`/`_WAK`, no BSP
// resume path through the FACS waking vector, no Kepler re-POST, no xHCI/AHCI save/restore) and the first entry each
// boot prints the firmware's half (`acpi_power::s3_census`) so the walls are named by fact. `idle` is the deepest
// state the tree holds honestly: the backlight off (gmux), the compositor and the cursor refused (`video::
// panel_refuse_term` term `sleep-panel-off`), so every core sits in the scheduler's `sti; hlt` (C1) between ticks;
// the 1 kHz APIC tick stays (the ms clock, the lid poll and every sleeper ride it; a tickless idle is unbuilt) and
// USB stays up (the FTDI cable is the wire and the keyboard is the input waker). Wakers, each NAMED: `lid` (MSLD back
// to 0, polled every `video::lidsleep::SLEEP_POLL_MS` while asleep), `power-key` (the fixed-feature PWRBTN_STS,
// `acpi_power::pwrbtn_take`), `input` (a key or pointer event; the Sleep button's route with the lid open).
// The owner of the panel half is `video/lidsleep.rs`; this module only records and prints.
#[cfg(all(target_arch = "x86_64", feature = "lidsleep", feature = "wc"))]
pub mod sleep {
    use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

    /// The state this tree enters.
    pub const STATE: &str = "idle";
    /// BP-bar-20: lid open -> first frame.
    pub const WAKE_BAR_MS: u64 = 1000;
    /// The named wakers (and `tests`, the fixture's own); any other source is unexplained.
    const SRCS: [&str; 4] = ["lid", "power-key", "input", "tests"];

    static ASLEEP: AtomicBool = AtomicBool::new(false);
    static ENTER_MS: AtomicU64 = AtomicU64::new(0);
    /// Battery at entry: `full << 16 | rem` (mAh); `u32::MAX` = not read.
    static ENTER_BATT: AtomicU32 = AtomicU32::new(u32::MAX);
    static WAKE_SRC: AtomicU32 = AtomicU32::new(0);
    static WAKE_AT_MS: AtomicU64 = AtomicU64::new(0);
    static WAKES: AtomicU32 = AtomicU32::new(0);
    static UNEXPLAINED: AtomicU32 = AtomicU32::new(0);
    static CENSUS: AtomicBool = AtomicBool::new(false);
    static STALE_PB: AtomicU32 = AtomicU32::new(0);

    /// The sleep state is in force (between [`enter`] and [`woke`]).
    pub fn asleep() -> bool { ASLEEP.load(Ordering::Acquire) }

    /// The state is entered (`via`: lid, loginwindow, tests …). Atomics and one line; the SMC and ACPI reads are
    /// [`measure_enter`]'s, on the service pass.
    pub fn enter(via: &str, backlight: &str) {
        if ASLEEP.swap(true, Ordering::AcqRel) { return; }
        ENTER_MS.store(crate::arch::ms().max(1), Ordering::Relaxed);
        ENTER_BATT.store(u32::MAX, Ordering::Relaxed);
        serial_println!("[power] sleep enter={} via={} backlight={} panel=refused cores=hlt-c1 tick=1khz usb=up s3=unreachable", STATE, via, backlight);
    }

    /// The entry's slow half, from the service pass after the panel is dark: the S3 census (once per boot), a stale
    /// power-button latch cleared (so it cannot wake the state it predates), and the battery at entry.
    pub fn measure_enter() {
        if !CENSUS.swap(true, Ordering::AcqRel) {
            match crate::arch::acpi_power::s3_census() {
                Some(c) => {
                    let s3 = match c.s3 { Some((a, b)) => alloc::format!("{},{}", a, b), None => alloc::string::String::from("absent") };
                    serial_println!(
                        "[power] s3 census facs={:#x} waking_vec={:#x} s3_pkg={} pts={} wak={} pwrbtn={} aml_interp=none bsp_resume=unwritten kepler_post=unwritten xhci_restore=unwritten -> s3=unreachable state={}",
                        c.facs, c.waking_vec, s3, c.pts as u8, c.wak as u8, crate::arch::acpi_power::pwrbtn_kind(), STATE
                    );
                }
                None => serial_println!("[power] s3 census acpi=absent -> s3=unreachable state={}", STATE),
            }
        }
        if crate::arch::acpi_power::pwrbtn_take() == Some(true) { STALE_PB.fetch_add(1, Ordering::Relaxed); }
        let (_, _, full, rem) = crate::drivers::smc::battery::extras();
        if let (Some(f), Some(r)) = (full, rem) { ENTER_BATT.store((f as u32) << 16 | r as u32, Ordering::Relaxed); }
        serial_println!("[power] sleep measured full_mah={} rem_mah={} stale_pwrbtn={}", opt(full), opt(rem), STALE_PB.load(Ordering::Relaxed));
    }

    /// A latched power-button press (the fixed-feature PWRBTN_STS), taken.
    pub fn power_key() -> bool { crate::arch::acpi_power::pwrbtn_take() == Some(true) }

    /// The state is left by `src`. Records the detect time; the line waits for [`first_frame`].
    pub fn woke(src: &str) {
        if !ASLEEP.swap(false, Ordering::AcqRel) { return; }
        let i = SRCS.iter().position(|s| *s == src);
        if i.is_none() { UNEXPLAINED.fetch_add(1, Ordering::Relaxed); }
        WAKE_SRC.store(i.map(|i| i as u32).unwrap_or(u32::MAX), Ordering::Relaxed);
        WAKE_AT_MS.store(crate::arch::ms().max(1), Ordering::Relaxed);
        WAKES.fetch_add(1, Ordering::Relaxed);
    }

    /// The wake's first composite has returned: print the wake line and the SLEEP verdict. The battery is read
    /// AFTER the frame, so the SMC never sits inside `wake_ms`. `hw`: the backlight answered (a gmux is there).
    pub fn first_frame(hw: bool) {
        let at = WAKE_AT_MS.swap(0, Ordering::AcqRel);
        if at == 0 { return; }
        let now = crate::arch::ms();
        let wake_ms = now.saturating_sub(at);
        let slept_ms = at.saturating_sub(ENTER_MS.load(Ordering::Relaxed));
        let src = SRCS.get(WAKE_SRC.load(Ordering::Relaxed) as usize).copied().unwrap_or("unexplained");
        let poll = crate::video::lidsleep::SLEEP_POLL_MS;
        serial_println!("[power] sleep enter={} wake src={} ms={} slept_ms={} lid_poll_ms={}", STATE, src, wake_ms, slept_ms, poll);
        let e = ENTER_BATT.load(Ordering::Relaxed);
        let (_, _, full, rem) = crate::drivers::smc::battery::extras();
        // Hundredths of a percent per hour: (rem0 - rem1) / full over the slept hours. Measured only across ≥ 60 s of
        // sleep with both reads and a falling or flat charge (on the adapter the pack charges: not a drain).
        let drain = match (e, full, rem) {
            (e, Some(f), Some(r1)) if e != u32::MAX && f != 0 && slept_ms >= 60_000 && (e & 0xFFFF) >= r1 as u32 => {
                let r0 = (e & 0xFFFF) as u64;
                Some((r0 - r1 as u64) * 10_000 * 3_600_000 / (f as u64 * slept_ms))
            }
            _ => None,
        };
        let why = match (e, full, rem) {
            _ if drain.is_some() => "measured",
            (u32::MAX, _, _) | (_, None, _) | (_, _, None) => "no-battery-keys",
            _ if slept_ms < 60_000 => "under-60s",
            _ => "charging",
        };
        let d = match drain { Some(h) => alloc::format!("{}.{:02}", h / 100, h % 100), None => alloc::string::String::from("unmeasured") };
        let unexpl = UNEXPLAINED.load(Ordering::Relaxed);
        let verdict = if !hw && crate::video::backlight::gmux_absent() {
            "SKIP reason=no-gmux (QEMU: no backlight, no lid key, no battery keys)"
        } else if unexpl == 0 && hw && wake_ms <= WAKE_BAR_MS {
            "PASS"
        } else {
            "FAIL"
        };
        serial_println!(
            ":: SLEEP: state={} wake_ms={} drain_pct_per_h={} wakes={} unexplained={} -> {} :: src={} slept_ms={} drain={} rem_mah={} lid_poll_ms={}",
            STATE, wake_ms, d, WAKES.load(Ordering::Relaxed), unexpl, verdict, src, slept_ms, why, opt(rem), poll
        );
    }

    fn opt(v: Option<u16>) -> alloc::string::String {
        match v { Some(x) => alloc::format!("{}", x), None => alloc::string::String::from("-") }
    }
}
