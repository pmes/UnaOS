// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use lazy_static::lazy_static;
use spin::Mutex;
use uart_16550::backend::PioBackend;
use uart_16550::Uart16550Tty;

lazy_static! {
    // `None` when there is no 16550 at 0x3F8. `new_port` runs init() + a loopback self-test and
    // returns Err on real laptops / the Pi (no UART there) — we must treat that as "no serial",
    // NEVER panic. The original bug: `.unwrap()` here panicked on metal, then the panic handler
    // called serial_println! which re-ran this initializer and recursed — a red screen with no
    // message and a freeze. fbcon mirrors all output to the framebuffer regardless.
    pub static ref SERIAL1: Mutex<Option<Uart16550Tty<PioBackend>>> = {
        let serial_port = unsafe { Uart16550Tty::new_port(0x3F8, uart_16550::Config::default()).ok() };
        Mutex::new(serial_port)
    };
}

/// Tri-state memory of whether this machine actually has a 16550 at 0x3F8, learned the first time the
/// lazy `SERIAL1` initializer runs. 0 = not yet known, 1 = present, 2 = absent.
///
/// Load-bearing for the staging ring: on a machine with NO serial port (a real laptop, the Pi) nothing
/// will ever drain the ring, so staging a contended line there would silently fill it and then produce
/// a stream of bogus `[serial] dropped N lines` markers about output that was never lost — fbcon
/// mirrors every one of those lines to the framebuffer regardless. On a serial-less machine the UART
/// path stays exactly what it always was: a no-op.
///
/// SERWIT-1D: it is also the CONFIGURATION the conservation law is asserted against. A line that this
/// machine's 16550 never received because there is no 16550 is neither emitted nor lost, and calling it
/// either one makes the ledger lie; see [`uart_absent`] and `serial_ring::DECLINED`.
static UART_STATE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// SERWIT-1D — has this machine been proven to have NO 16550 at 0x3F8?
///
/// `false` while the answer is still unknown (before the first `_print` resolves the lazy `SERIAL1`),
/// which is the conservative reading: "unknown" must not license the weaker of the two configuration
/// laws. `_print` runs long before any fixture, so by the time [`crate::serial_ring::serwit_verdict`]
/// asks, the answer is settled and stays settled — `SERIAL1` is initialised once and never re-probed.
#[inline]
pub fn uart_absent() -> bool {
    UART_STATE.load(core::sync::atomic::Ordering::Relaxed) == 2
}

#[doc(hidden)]
pub fn _print(args: ::core::fmt::Arguments) {
    use core::fmt::Write;
    use core::sync::atomic::Ordering;
    use x86_64::instructions::interrupts;

    crate::serial_ring::note_submitted();

    // PANIC ESCAPE HATCH. Past `enter_panic_mode` the Mutex is not touched at all: the machine is
    // dying, this may well be the very core that owns `SERIAL1` (the historical self-deadlock the
    // `try_lock` was introduced to survive — at the cost of eating the panic message outright), and a
    // last-words path that can lose its words is not a last-words path. `raw_write_str` is the same
    // lock-free bounded-poll sequence WEDGE-2 uses, so it acquires nothing and cannot deadlock; it is
    // synchronous, so the bytes are on the wire before the next statement runs. Drain the staged
    // backlog first so lines queued just before the fault are not buried with the machine.
    if crate::serial_ring::in_panic_mode() {
        crate::serial_ring::drain(raw_write_str);
        let mut raw = RawUart;
        let _ = raw.write_fmt(args);
        crate::serial_ring::note_emitted();
        crate::video::fbcon::_print(args);
        crate::drivers::xhci::ftdi::mirror(args);
        crate::selftest::capture(args);
        crate::flight_recorder::capture(args);
        return;
    }

    // ── SERIALTX (rmbp-ledger B154) — THE MASKED REGION IS BYTE-FREE, AND THE EMISSION IS NOT IT ──
    //
    // WHAT THIS REPLACED, and why it had to go. Until this arc the whole of the paragraph below ran
    // inside ONE `interrupts::without_interrupts`: the winner of `SERIAL1.try_lock()` drained the
    // staging ring (capped at `DRAIN_BYTE_BUDGET` = 192 B since SO31/DRAINCAP) and then wrote ITS OWN
    // LINE synchronously, byte by byte, polling LSR bit 5 before each `out` to the THR. At 115200 8N1
    // that is 86.8 us per byte — **~8.7 ms for a 100-byte line, with interrupts masked on that core**,
    // on top of up to 22.6 ms of capped drain. A core inside that span cannot be preempted and runs no
    // service pass, which is exactly what EHCIDARK measured from the other end on flight 11 (B146,
    // `docs/dev/OS/07_USB_STORAGE/usb_xhci.md` §33h): the EHCI HID pass period collapses from 1.08 ms
    // mean across 840 quiet seconds to 6.46 ms across the 17 burst seconds, `max=108ms` lives inside
    // that, and the deadman's 1 Hz line on ANOTHER core stretches to 2041 ms in the same seconds.
    //
    // THE SHAPE NOW, in the order it runs:
    //
    //   (a) MASKED, BYTE-FREE. Resolve the 16550 tri-state if it is still unknown, and ENQUEUE this
    //       line into the lock-free staging ring. No `out`, no LSR poll, no UART lock held across a
    //       byte: the region is a compare-exchange and a memcpy. `SERIAL1` is taken here only to ASK
    //       whether a 16550 exists, once per boot, and on the no-16550 machine to hold the drainer's
    //       uniqueness across `discard_staged` — the SERWIT-1D accounting below is unchanged.
    //
    //   (b) THE DRAIN OWNER, WITH INTERRUPTS ENABLED. Whichever core printed next has always been the
    //       drain owner; the change is that it now drains UNMASKED. The three candidates the brief
    //       named were the 1 kHz timer tick, the 1 ms device-service pump, and this one; this one is
    //       the one that needs no line in a file this arc may not touch, and it is not a fallback on
    //       the merits — it is self-clocking (a console under load has, by definition, a next print
    //       arriving), it needs no core to be reserved, and it keeps the drain on the core that is
    //       already paying for the console instead of moving that cost onto the service core. The
    //       ring's residual flush when printing STOPS is `serial_ring::mirror_service`, which the
    //       main loop polls and whose contract is already IF=1 and lock-free.
    //
    //   (c) A print that arrives ALREADY MASKED — from an interrupt handler, an exception, or inside
    //       another subsystem's `without_interrupts` — cannot unmask, because re-enabling interrupts
    //       is not the console's to do. It still owes the ring forward progress, so it takes exactly
    //       ONE 16550 transmit FIFO (`serial_ring::MASKED_BYTE_BUDGET` = 16 B = 1.39 ms worst case)
    //       and leaves the rest. That is the ceiling this arc was asked for, and `sertx_selftest`
    //       asserts the ordinary unmasked path spends ZERO of it.
    //
    // ORDERING IS UNCHANGED AND IS NOW STRUCTURAL. The old code drained other cores' staged lines
    // before writing its own directly, so that a line staged at t0 preceded a line written at t1 > t0.
    // Now every line goes through the ring, so wire order IS submission order by construction and
    // there is no "direct" line left to reorder against.
    //
    // LOSS IS UNCHANGED. SERWIT-1's contract is untouched, branch for branch: `defer_contended` is
    // still the single shared policy, a full ring still back-pressures for `BACKPRESSURE_SPINS`
    // bounded turns, and a line that outlives the bound is still counted in `DROPPED` and announced on
    // the wire by the next drain. What changed inside the retry turn is WHERE it makes progress: the
    // old turn re-tried the UART *inside the mask* and, winning it, paid for a whole capped drain
    // there; the new turn makes room by running the drain owner OUTSIDE it.
    let mut spins: u32 = 0;
    let mut masked_sum: u64 = 0;
    let mut masked_max: u64 = 0;
    let mut drain_cyc: u64 = 0;
    let mut spin_cyc: u64 = 0;
    let spin_t0 = crate::arch::now_cycles();
    loop {
        let m0 = crate::arch::now_cycles();
        // `true` => this line's fate is settled (staged, declined, or lost past the bound).
        let settled = interrupts::without_interrupts(|| {
            match UART_STATE.load(Ordering::Relaxed) {
                0 => {
                    // The lazy probe has not run. Ask — and only ask; nothing is emitted here.
                    if let Some(guard) = SERIAL1.try_lock() {
                        let present = guard.is_some();
                        UART_STATE.store(if present { 1 } else { 2 }, Ordering::Relaxed);
                        crate::serial_ring::note_uart_resolved();
                        if !present {
                            // No 16550 on this machine. Nothing was written, nothing was lost that
                            // fbcon and the FTDI mirror below do not already carry — and there is no
                            // reason to keep a backlog nobody will ever drain. The guard is still held,
                            // which is what makes this the unique drainer for `discard_staged`.
                            //
                            // SERWIT-1D: this outcome is DECLINED, and it is neither of the other two.
                            // It is not `emitted` — no byte reached a 16550, because there is none —
                            // and calling it emitted is the lie that `drain(|_| {})` used to tell,
                            // since `drain` charges `EMITTED` for every line it consumes and this sink
                            // is `|_| {}`. Nor is it `dropped`: the line is on the wire via the FTDI
                            // mirror, whose own conservation law is asserted separately by SERWIT-2's
                            // `ftdi` tap. So it gets its own terminal state and its own counter.
                            crate::serial_ring::note_declined();
                            crate::serial_ring::discard_staged();
                            return true;
                        }
                    }
                    // Contended while still unknown: fall through and stage, exactly as before. The
                    // window is narrow and the lines staged in it are the ones `discard_staged`
                    // charges to `DECLINED` the moment the answer turns out to be "absent".
                }
                2 => {
                    // SERWIT-1D: there is no 16550 — the branch that had no counter at all before that
                    // arc. Staging here would fill a ring nobody drains, so the line correctly goes
                    // nowhere on THIS transport; what was missing is the accounting for it. Checked
                    // BEFORE the stage attempt so this machine never back-pressures against a ring
                    // that has no consumer.
                    crate::serial_ring::note_declined();
                    return true;
                }
                _ => {}
            }
            // SERWIT-1B PARITY (SO31): `try_stage` succeeded / the bound expired / go round are ONE
            // call, because aarch64 needs the identical decision and two copies of a policy is how two
            // divergent policies happen. `serial_ring::defer_policy` is the single pure decision and
            // its rows are asserted at compile time on both arches.
            !matches!(
                crate::serial_ring::defer_contended(args, &mut spins),
                crate::serial_ring::Defer::Retry
            )
        });
        let m = crate::arch::now_cycles().wrapping_sub(m0);
        masked_sum = masked_sum.wrapping_add(m);
        if m > masked_max {
            masked_max = m;
        }
        if settled {
            break;
        }
        // The ring is full and the bound has not expired. Make room OUTSIDE the mask — this is the
        // progress-bearing half of SERWIT-1B's turn, moved off the masked path.
        drain_cyc = drain_cyc.wrapping_add(drain_owner());
    }
    if spins > 0 {
        crate::serial_ring::note_stalled(spins);
        // The cost of contention for this print, retry turns and their drains included. Zero on an
        // uncontended print, which is every print on a machine that is not flooding its console.
        spin_cyc = crate::arch::now_cycles().wrapping_sub(spin_t0);
    }
    // (b) — the emission, with interrupts ENABLED. `emit_cyc` is charged 0 and that is the point of
    // the whole arc: this arch no longer writes its own line synchronously, so the term B146 named
    // has no site left to be spent at. `arch/aarch64/serial.rs` still does, and its `[sertx] emit_us`
    // is what says so.
    drain_cyc = drain_cyc.wrapping_add(drain_owner());
    crate::serial_ring::tx_charge(masked_sum, masked_max, drain_cyc, 0, spin_cyc);
    // SERIALTX: the four POST-MASK taps are timed as one block and reported as `[sertx] taps_us`.
    // They are NOT part of the masked census above and must not be folded into it — but they are not
    // free either, and on the bench rMBP they are the ONLY term that can be large, because that
    // machine has no 16550 at all (`uart16550=absent carrier=ftdi-mirror law=emitted==0` on flight
    // 11) and every masked branch above is therefore the O(1) DECLINED one. A census that measured
    // only the mask would have acquitted the console on the one board the arc was commissioned for.
    let taps_t0 = crate::arch::now_cycles();
    // Mirror to the framebuffer console so diagnostics/panics are visible on hardware that has
    // no serial port. `Arguments` is Copy; fbcon self-guards (try_lock + interrupts off).
    crate::video::fbcon::_print(args);
    // U2.5: mirror into the FTDI console boot-capture ring — ALWAYS, from the very first print, so
    // when the USB-serial console comes up mid-boot the whole early log replays out the cable. The
    // ring self-guards (try_lock only, never blocks, drop-oldest on overflow); it never takes the
    // XHCI_CONTROLLER lock or allocates, so this is safe from any print context.
    crate::drivers::xhci::ftdi::mirror(args);
    // TSTE-1 M2b: capture boot-fixture verdict lines (`-> PASS`/`-> FAIL`) into the selftest ring so
    // `tste` can replay them. Additive, alloc-free, `try_lock` only; safe from this IRQ-masked
    // context; zero change to what is printed above.
    crate::selftest::capture(args);
    // FLIGHT-RECORDER: capture the exact serial line bytes into the boot-log ring so `service()` can
    // later flush the whole boot log to UNAOS.LOG on the FAT volume. Same discipline as the taps
    // above — additive, alloc-free, `try_lock` only, drop-on-full; zero change to what is printed.
    crate::flight_recorder::capture(args);
    crate::serial_ring::tx_charge_taps(crate::arch::now_cycles().wrapping_sub(taps_t0));
}

/// SERIALTX (rmbp-ledger B154) — **the drain owner**: emit from the staging ring at the 16550, and
/// return what it cost in `arch::now_cycles()` units.
///
/// Two budgets, and the interrupt flag picks between them **as it actually stands**, never by which
/// call site the author believed they were on:
///
///   * **interrupts ENABLED** — `DRAIN_BYTE_BUDGET` (192 B, one 60 Hz frame of UART; SO29/DRAINCAP).
///     This is the ordinary path and it can be preempted at any byte, so its cost is charged to
///     throughput and not to latency. Every byte it writes is charged to `[sertx] bytes` and to the
///     UNMASKED half of that ledger.
///   * **interrupts MASKED** — `MASKED_BYTE_BUDGET` (16 B, one 16550 transmit FIFO). Reached only by
///     a print that ARRIVED masked; re-enabling is not the console's to do. The remainder stays in
///     the ring, in order, for the next unmasked owner. Its bytes are charged to the MASKED half,
///     which is the number `sertx_selftest` asserts is zero on the ordinary path and the number the
///     go-red turns into the line cost.
///
/// `try_lock` (never `lock`) is still the rule and is what keeps the drainer unique: a print from any
/// context that loses it simply leaves the ring to the next owner, and the panic path never comes
/// here at all.
///
/// `pub` for ONE second caller: `serial_ring::residual_drain`, the main loop's flush. A staged line is
/// not a lost one only for as long as there is a next print (the argument PWRDRAIN is built on), and
/// with the emission moved off the print's own masked path the tail of a burst can outlive the print
/// that queued it by one drain. The main-loop poll closes that window without a foreign file.
pub fn drain_owner() -> u64 {
    use core::fmt::Write;
    use core::sync::atomic::Ordering;
    use x86_64::instructions::interrupts;
    if UART_STATE.load(Ordering::Relaxed) != 1 {
        return 0;
    }
    let t0 = crate::arch::now_cycles();
    if let Some(mut guard) = SERIAL1.try_lock() {
        if let Some(uart) = guard.as_mut() {
            let mut sink = |s: &str| {
                crate::serial_ring::tx_note_bytes(s.len());
                #[cfg(feature = "logts")]
                {
                    let _ = crate::logts::PrefixWriter { inner: uart }.write_str(s);
                }
                #[cfg(not(feature = "logts"))]
                {
                    let _ = uart.write_str(s);
                }
            };
            // ⚠ THE GO-RED FOR `sertx_selftest` IS THIS `if`: wrap the `drain_capped` arm in
            // `interrupts::without_interrupts(...)` and the same sink's bytes re-file themselves as
            // masked, `:: SERIALTX:` reads `masked_b=201` and FAILs, and `[sertx] masked_us_max=`
            // returns to the line cost in the same run.
            if interrupts::are_enabled() {
                crate::serial_ring::drain_capped(&mut sink);
            } else {
                crate::serial_ring::drain_fifo(&mut sink);
            }
        }
    }
    crate::arch::now_cycles().wrapping_sub(t0)
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::arch::serial::_print(format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! serial_println {
    () => ($crate::arch::serial::_print(format_args!("\n")));
    ($($arg:tt)*) => ($crate::arch::serial::_print(format_args!("{}\n", format_args!($($arg)*))));
}

/// One byte at the 16550, **taking no lock**: a bounded poll of `LSR` bit 5 (transmitter holding
/// register empty) at `0x3F8 + 5`, then one `out` to the THR at `0x3F8`.
///
/// This is the tree's single lock-free UART write primitive, shared by two callers with the same hard
/// requirement — that they cannot block on anything:
///   * WEDGE-2/WEDGE-4 breadcrumbs (see [`wedge2_raw_byte`] and `crate::wedge2`), which must survive a
///     core dying with IRQs masked while holding any of `SERIAL1`/`FBCON`/`WRITER`/the allocator;
///   * the panic escape hatch in [`_print`], which must emit synchronously even when the panicking
///     core is itself the owner of `SERIAL1`.
///
/// It acquires NOTHING and allocates nothing. The spin is bounded so a machine with no 16550 degrades
/// (bytes into the void) instead of hanging — the same bound the aarch64 twin carries.
#[inline(never)]
pub fn raw_byte(byte: u8) {
    use x86_64::instructions::port::Port;
    unsafe {
        let mut lsr: Port<u8> = Port::new(0x3F8 + 5);
        let mut thr: Port<u8> = Port::new(0x3F8);
        let mut spins: u32 = 0;
        while (lsr.read() & (1 << 5)) == 0 {
            spins += 1;
            if spins > 1_000_000 {
                break;
            }
            core::hint::spin_loop();
        }
        thr.write(byte);
    }
}

/// A `core::fmt::Write` over [`raw_byte`] — lock-free formatting straight at the UART, for the panic
/// path only. Deliberately NOT used on any ordinary print: because it takes no lock, its bytes can
/// interleave with another core's in-progress line, which is the right trade for last words and the
/// wrong one for a `PASS` tally.
pub struct RawUart;

impl core::fmt::Write for RawUart {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for b in s.bytes() {
            raw_byte(b);
        }
        Ok(())
    }
}

/// Free-function form of [`RawUart`]'s writer, shaped for `serial_ring::drain`'s `FnMut(&str)` sink.
pub fn raw_write_str(s: &str) {
    for b in s.bytes() {
        raw_byte(b);
    }
}

/// WEDGE-2 — the x86_64 half of the breadcrumb seam: one byte at the UART, **taking no lock**.
///
/// Deliberately NOT `SERIAL1`: that is a `Mutex<Option<Uart16550Tty<_>>>`, and a breadcrumb whose job
/// is to survive a core dying with IRQs masked must never be able to block. This is the bare 16550
/// sequence instead — a bounded poll of `LSR` bit 5 (transmitter holding register empty) at
/// `0x3F8 + 5`, then one `out` to the THR at `0x3F8`. It acquires nothing: not `SERIAL1`, not `FBCON`,
/// not the video locks, not the allocator — all of which are reachable from the focus chain WEDGE-2
/// instruments. The spin is bounded so a machine with no 16550 degrades instead of hanging.
///
/// s44 (the x86 reproduction, capture truncated mid-word) is why this exists on this arch at all: the
/// mechanism is arch-neutral, so the instrumentation has to be portable and only this function is not.
/// See `crate::wedge2` for the token table and the interleaving trade-off.
///
/// SERWIT-1 note: the body moved verbatim into [`raw_byte`] so the panic escape hatch could share the
/// one audited lock-free sequence. Nothing about WEDGE-2's contract changed — same bounded LSR poll,
/// same single `out`, still no lock of any kind, and the staging ring this file gained is invisible
/// from here (breadcrumbs never enter `serial_ring`, so there is no new lock they could contend for).
#[cfg(feature = "wedge2")]
#[inline(never)]
pub fn wedge2_raw_byte(byte: u8) {
    raw_byte(byte);
}

/// PFWIRE self-test — feature `pfwire_selftest`, armed by `UNAOS_PFWIRE_SELFTEST=1`.
///
/// **BRICKS THE BOOT BY DESIGN.** It deliberately provokes a fatal CPL-0 page fault to prove the
/// `#PF` handler puts its diagnostics on the wire, so it is compiled out by default and must NEVER
/// ride boot media or the regression battery (both leave the knob unset). Called from `arch::init`
/// with IF=0, after `init_idt()` (the handler is installed) and after `wxn_pdpt_sweep()` — the exact
/// interrupt-masked, kernel-context window an M3b wild write would fault in.
///
/// It forces the CONTENDED case, the only one that discriminates the fix. An *uncontended* CPL-0 #PF
/// prints its address fine with or without `enter_panic_mode` — the handler's `serial_println!` wins
/// `SERIAL1.try_lock()` and drains normally — so it cannot tell the two apart. Here we take `SERIAL1`
/// and leak the guard, then fault while holding it. WITHOUT the fix the handler's four lines lose the
/// `try_lock`, `try_stage` into a ring nobody will drain (this core is about to `hlt` with IF=0), and
/// the machine is silent on serial. WITH the fix `enter_panic_mode()` has already switched `_print`
/// to `RawUart`'s lock-free synchronous writes, so `EXCEPTION: PAGE FAULT` / `Accessed Address:` reach
/// the wire regardless of the leaked lock. The wild store targets a canonical higher-half address the
/// identity-mapped kernel never maps, so it is a not-present #PF (not a #GP), independent of the WXN map.
#[cfg(feature = "pfwire_selftest")]
pub fn pfwire_selftest() -> ! {
    // Announced on the FREE lock, BEFORE we take it — this line lands in both the armed build and the
    // fix-reverted comparison build, so the A/B reads it as the "we reached the test" marker.
    serial_println!("PFWIRE-SELFTEST: forcing a CONTENDED CPL-0 #PF (SERIAL1 held); boot will halt");
    // Take SERIAL1 and abandon the guard so the fault handler's `try_lock` is guaranteed to lose.
    if let Some(g) = SERIAL1.try_lock() {
        core::mem::forget(g);
    }
    // Canonical higher-half address the identity-mapped kernel never maps -> a clean not-present #PF.
    const PFWIRE_WILD: u64 = 0xFFFF_DEAD_0000_0000;
    unsafe {
        core::ptr::write_volatile(PFWIRE_WILD as *mut u64, 0xB00B);
    }
    // Unreachable: the store above faults into `page_fault_handler`, which never returns. Kept so the
    // `-> !` signature holds even if the optimizer cannot prove the store faults.
    crate::hlt_loop()
}
