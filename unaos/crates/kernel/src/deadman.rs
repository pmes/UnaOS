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

//! `[deadman]` — THE INSTRUMENT THAT SURVIVES THE WEDGE.
//!
//! ## The defect this exists for
//! Metal boot 11 wedged at 117.668 s and the glass stayed frozen for the last 91 seconds — 43 % of
//! the operator's session — and there is **no data for that stretch**. `spread`, `rtwit`, `schedx86`,
//! `dock` and `menubar` all went silent in ONE event, not a cascade. That is structural, not bad
//! luck: every one of those instruments is emitted BY the render-service pass, so when the pass
//! stopped, so did the evidence of it stopping. `shell.rs` already names the gap in as many words —
//! *"on x86 there is NO instrument that survives a starved shell (the `[schedx86] load` heartbeat
//! runs on the same render-service task as this dispatch; on aarch64 the timer-driven
//! `:: SCHED: load ::` / `[pulse5]` / `[spin1]` train does survive)"*. This module is the x86 half of
//! that train.
//!
//! ## The shape
//! Driven from the **APIC timer ISR** (`arch::x86_64::interrupts::timer_interrupt_handler`), not from
//! any service pass. One line per second, **unconditionally — including when every counter is zero**,
//! so that SILENCE IS DISTINGUISHABLE FROM IDLENESS. A `[deadman]` line reading all zeros says "the
//! machine is alive and nothing is happening". No `[deadman]` line at all says "the timer ISR or the
//! console transport is gone", which is a different and much larger claim. Neither reading was
//! available for boot 11's missing 91 seconds.
//!
//! ## The line
//! ```text
//! [deadman] up=117 hid=42 pmp=998 hq=3 hid_ms=118 comp_ms=91234 gate=2/8891 dec=00200000 dmg=3/3
//! ```
//! | field | meaning |
//! |---|---|
//! | `up=` | seconds since boot (`APIC_TICKS / 1000`), the anchor a capture is read against |
//! | `hid=` | HID IN-token completions in the last second (see HID-IN GAP below) |
//! | `pmp=` | EHCI HID **poll passes** in the last second — the poll's own liveness |
//! | `hq=` | input event-queue depth, read lock-free via `pal`'s conservation law |
//! | `hid_ms=` | age of the most recent HID report, `-` if none ever |
//! | `comp_ms=` | age of the last **completed** composite pass, `-` if none ever |
//! | `gate=` | `COMP_GATE` holder core `/` hold age in ms; `-` when the gate is free |
//! | `dec=` | per-core composite DECLINES in the last second, one hex nibble per core, saturating at `f` |
//! | `dmg=` | rows currently marked damaged `/` of those, rows with NO attach since the last pass |
//! | `in=` | INPUT-UNGATE: motion reports coalesced `/` channel sends declined `/` heartbeats dropped, in the last second |
//! | `rh=` | WCSER-REHOME: singleton roles re-homed off a steal-declared-dead core this boot (standing, never drained) |
//!
//! Roughly 95 bytes. That is deliberate: `USB-DEBUG: MOUSE` was measured at 49 bytes per report and
//! was 20.8 % of ALL serial output before it was removed. One 95-byte line per second is ~0.8 % of a
//! 115200-baud wire and cannot repeat that mistake.
//!
//! ## The two questions it decides
//! **(a) Was input actually dead after the wedge, or did the operator stop trying?** Read `hid` and
//! `pmp` TOGETHER — that pairing is the whole answer and neither field decides it alone:
//! * `pmp > 0`, `hid = 0`, `comp_ms` growing without bound ⇒ the poll is alive and the hardware
//!   genuinely reported nothing. Only the glass is dead; the input path still runs.
//! * `pmp > 0`, `hid > 0`, `comp_ms` growing ⇒ input alive AND arriving. The operator was still
//!   typing into a dead screen.
//! * `pmp = 0` ⇒ the EHCI poll itself stopped, so this instrument can say NOTHING about whether the
//!   hardware still had reports — it can only say that the pass which would have harvested them is
//!   gone. That is the materially worse bug, and it is now nameable instead of invisible.
//!
//! **(b) Are the phantom composites a damage-tracking leak?** `dmg=` is the adjudicator. In the
//! 40.8–105 s stretch the screen was provably static yet the compositor did 1.56 composites per
//! attach where boots 9/10 do exactly 1.00. `dmg_rows_static > 0` on a static screen proves rows are
//! being carried into a pass with no attach behind them — a leak — and the number quantifies it.
//! `dmg=N/0` says the extra passes are NOT a damage leak and the 1.56 must be explained elsewhere.
//!
//! **A sibling executor owns FIXING that defect. This module only adjudicates it.** The counter is
//! built to the definition, not toward a hoped-for answer: `dmg_rows_static` counts a row as static
//! on the strict test `attach_seq[i] == attach_seq_at_last_pass[i]`, which is false the instant any
//! `present()` names the row, and no threshold, decay or smoothing is applied to either term.
//!
//! ## ⚠ HID-IN GAP — NAMED, NOT PAPERED OVER
//! The brief asked for `hid_in` incremented **inside the EHCI IRQ handler, before any queue or
//! lock**. *That site does not exist on this machine.* `drivers/ehci/mod.rs:19` is explicit: **"No
//! interrupts: no USBINTR write, no IDT vector, no MSI."** The rMBP's internal keyboard and trackpad
//! are serviced by `service_ehci_hid()`, POLLED from the `x86_usb_pump` task, and `USBINTR` is never
//! written, so no EHCI interrupt is ever delivered and there is no IRQ-side counter to keep.
//!
//! Rather than weaken the requirement by pretending a polled call site is an ISR, the field is
//! SPLIT so the gap is visible in the reading itself:
//! * `hid=` is stamped at the EARLIEST honest point — the qTD retirement at `ehci/mod.rs:12536`,
//!   above every decoder, length gate and `dead` path, and before ANY `pal::EVENT_QUEUE` push — so
//!   no report layout can influence whether a completion counts.
//! * `pmp=` is stamped once per `service_ehci_hid()` entry, so the reader can always tell whether
//!   `hid = 0` means "the hardware said nothing" or "nobody asked the hardware".
//!
//! The residual is stated plainly: **when `pmp = 0` this instrument cannot see HID hardware
//! liveness at all.** Closing it needs a read-only sample of the EHCI `USBSTS.USBINT` bit (which
//! `QTD_IOC` still sets even with `USBINTR` masked) taken from the timer ISR off a base address
//! cached at init. That is a separate rung and is deliberately NOT built here — it puts an MMIO read
//! of a device BAR inside the timer ISR, which needs its own power-state and mapping argument.
//!
//! ## ⚠ `ser_wait[0..7]` — DROPPED, AND WHY
//! The brief asked for a per-core count of threads **blocked on** the compositor's serialization
//! lock. **No such set exists, because `COMP_GATE` is not a lock.** It is an `AtomicBool` gate
//! (`video/wm.rs:7748`) taken with `compare_exchange` and, on failure, DECLINED — the caller
//! publishes `COMP_PENDING` and returns immediately (`wm.rs:3835`). Nothing ever blocks on it, so a
//! "threads blocked" count would be a field that is structurally always zero and would read as
//! "nobody was waiting" during precisely the wedge it was built to explain.
//!
//! `dec=` is the honest analogue with the same job: a per-core count of composite passes TURNED AWAY
//! by the gate. It answers the question the brief actually posed — it converts "nothing ran right"
//! into an enumerated list of which cores tried and were refused — while naming the real mechanism.
//! During a leaked hold, `dec=` is exactly the roster of cores that kept arriving and kept being sent
//! away, and `gate=` names the core that never let go.
//!
//! ## Safety — why no field can block
//! The full per-field argument lives in `docs/dev/OS/08_VIDEO/engine.md` §DEADMAN. The short form:
//! every field is either a `Relaxed` load of an atomic this module or `wm`/`apic` already maintains,
//! or a `TABLE.try_lock()` that reports `?` and moves on. **This module never calls
//! `wm::composite()`, never touches `COMP_GATE`, never calls `pal::event_queue_depth()` (which locks),
//! and never calls `wm::table()` (a blocking IRQ-masked spin).** An instrument that can block on what
//! it instruments is the bug, not the tool.
//!
//! ## Gating — zero cost when off
//! Behind `feature = "deadman"` (`UNAOS_DEADMAN=1`) AND `target_arch = "x86_64"`. When off, every
//! entry point is an empty `#[inline(always)]` shim, so the hooks in `arch/x86_64/interrupts.rs`,
//! `drivers/ehci/mod.rs` and `video/wm.rs` stay `#[cfg]`-free and a knob-off kernel carries no
//! `[deadman]` string and no counter. Follows the `rtwit`/`wedge2` pattern.

/// How many cores `dec=` enumerates. The rMBP is 8-core; a core index at or above this is folded
/// into the last nibble rather than dropped, so a wider machine over-reports slot 7 instead of
/// losing the sample silently.
pub const CORES: usize = 8;

/// Window ids `dmg=` tracks — must match `video::wm::MAX_WINDOWS`. Asserted below rather than
/// imported so this module has no compile-time dependency on the compositor's layout.
pub const ROWS: usize = 12;

// ─────────────────────────────────────────────────────────────────────────────────────────────
// REAL IMPLEMENTATION — x86 with the knob armed.
// ─────────────────────────────────────────────────────────────────────────────────────────────
#[cfg(all(feature = "deadman", target_arch = "x86_64"))]
mod imp {
    use super::{CORES, ROWS};
    use core::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};

    const _: () = assert!(ROWS == crate::video::wm::MAX_WINDOWS);

    /// The "never happened" stamp. `0` is a legal `arch::ms()` reading for the first millisecond of
    /// the boot, so a separate sentinel is needed to keep "no HID report has EVER arrived" distinct
    /// from "a HID report arrived at t=0" — the same distinction `rtwit` draws with `--`.
    const NEVER: u64 = 0;

    // ── the per-second counters, all drained at each emit ──────────────────────────────────────
    /// HID IN-token completions since the last line. See the HID-IN GAP note in the module header:
    /// this is a qTD retirement, not an interrupt, because this driver takes no interrupts.
    static HID_IN: AtomicU64 = AtomicU64::new(0);
    /// `service_ehci_hid()` entries since the last line — the POLL's own liveness, the term that
    /// stops `hid=0` from being read as "input is dead" when it means "nobody looked".
    static HID_POLL: AtomicU64 = AtomicU64::new(0);
    /// Per-core composite declines since the last line. **Single-writer per core**: core `i` is the
    /// only writer of slot `i`, so these never contend and never need a read-modify-write barrier
    /// beyond `Relaxed`.
    #[allow(clippy::declare_interior_mutable_const)]
    const DEC_INIT: AtomicU32 = AtomicU32::new(0);
    static DEC: [AtomicU32; CORES] = [DEC_INIT; CORES];

    // ── INPUT-UNGATE: what the input producer had to do about a channel it could not fill ───────
    //
    // These three live HERE rather than beside their counters in `main.rs` for the reason the module
    // header gives: the only emitter proven to survive the wedge is the timer ISR, and a witness for
    // a wedge that is printed by a task the wedge can starve is not a witness. `GUI_SENT_X86` /
    // `GUI_RECV_X86` / `GUI_FOLD_X86` already exist in `main.rs` and are reported by the
    // `[schedx86] depth` line — which the RENDER TASK prints, i.e. exactly the thing that was dead
    // for the 497 seconds this arc is about. Nothing in that ledger reached the log after 83.8 s.
    /// Relative-motion reports SUMMED into the producer's accumulator because the channel refused
    /// them. Conserved travel, not lost travel — see `x86_input_service`.
    static GUI_COALESCED: AtomicU64 = AtomicU64::new(0);
    /// `Channel::try_send` refusals on `GUI_CHANNEL_X86`. Counts ATTEMPTS refused, not events lost —
    /// a producer holding one owed event re-offers it every ~1 ms pass, so a channel that stays full
    /// for a whole second reads near the producer's pass rate (~10^3) rather than near the number of
    /// events behind it. That is the intended scale: single digits mean the render task stuttered,
    /// hundreds mean it is badly behind, and a reading pinned at the pass rate second after second
    /// means it is GONE — which is the state `rh=` then says whether anything was done about.
    static GUI_DECLINED: AtomicU64 = AtomicU64::new(0);
    /// `Event::Timer` heartbeats dropped rather than queued. Pure filler by contract, so this is the
    /// one class the producer is allowed to lose — counted anyway, because "allowed to lose" and
    /// "lost silently" are the distinction this whole module exists to keep.
    static GUI_TIMER_DROP: AtomicU64 = AtomicU64::new(0);

    /// WCSER-REHOME: singleton roles re-homed off a core the steal declared dead. NEVER drained —
    /// a standing nonzero is the session having survived something that used to end it, the same
    /// reading discipline `wm::COMP_STEALS` uses.
    static REHOMED: AtomicU64 = AtomicU64::new(0);

    // ── the gauges, never drained ──────────────────────────────────────────────────────────────
    /// `arch::ms()` of the most recent HID IN-token completion, or [`NEVER`].
    static HID_LAST_MS: AtomicU64 = AtomicU64::new(NEVER);
    /// `arch::ms()` of the last COMPLETED composite pass, or [`NEVER`]. Stamped at the tail of
    /// `composite_once`, after the pass has actually finished — a pass that started and never
    /// returned must NOT advance this, because "the compositor is stuck inside a pass" is exactly
    /// the state `comp_ms` exists to make visible.
    static COMP_LAST_MS: AtomicU64 = AtomicU64::new(NEVER);

    // ── the damage-leak adjudicator ────────────────────────────────────────────────────────────
    /// Per-row attach sequence: bumped once per `present()` that named the row, outside the table
    /// lock. Wraps; only equality against [`ATT_AT_PASS`] is ever tested, so a wrap costs at worst
    /// one misread row after 2^32 presents on one window.
    #[allow(clippy::declare_interior_mutable_const)]
    const ATT_INIT: AtomicU32 = AtomicU32::new(0);
    static ATT: [AtomicU32; ROWS] = [ATT_INIT; ROWS];
    /// Snapshot of [`ATT`] taken at the end of each completed composite pass. A row whose live
    /// sequence still equals its snapshot has had NO attach since that pass — if it is nonetheless
    /// still marked damaged, that damage is not backed by an attach.
    static ATT_AT_PASS: [AtomicU32; ROWS] = [ATT_INIT; ROWS];

    // ── the once-per-second gate ───────────────────────────────────────────────────────────────
    /// `APIC_TICKS` value at which the next line is due. The BSP ISR is the only writer.
    static NEXT_DUE_MS: AtomicU64 = AtomicU64::new(1000);
    /// Ticks between lines. 1 kHz timer, so 1000 ticks == one second.
    const PERIOD_MS: u64 = 1000;

    // ── hooks (hot paths) ──────────────────────────────────────────────────────────────────────

    /// One EHCI HID IN-token completion. Called from the qTD-retirement point in
    /// `drivers/ehci/mod.rs`, above every decoder and before any `pal` queue push.
    #[inline]
    pub fn note_hid_completion() {
        HID_IN.fetch_add(1, Relaxed);
        // `.max(1)` keeps a t=0 completion from reading as NEVER.
        HID_LAST_MS.store(crate::arch::ms().max(1), Relaxed);
    }

    /// One `service_ehci_hid()` entry.
    #[inline]
    pub fn note_hid_poll() {
        HID_POLL.fetch_add(1, Relaxed);
    }

    /// One `present()` that named row `id` (1-based `WinId`). Outside the table lock.
    #[inline]
    pub fn note_attach(id: u32) {
        let i = id as usize;
        if i >= 1 && i <= ROWS {
            ATT[i - 1].fetch_add(1, Relaxed);
        }
    }

    /// One composite pass COMPLETED. Stamps `comp_ms`'s clock and snapshots the attach sequences
    /// the next `dmg=` reading is measured against.
    #[inline]
    pub fn note_composite_done() {
        COMP_LAST_MS.store(crate::arch::ms().max(1), Relaxed);
        for i in 0..ROWS {
            ATT_AT_PASS[i].store(ATT[i].load(Relaxed), Relaxed);
        }
    }

    /// One composite pass DECLINED by `COMP_GATE`. Charged to the declining core.
    #[inline]
    pub fn note_decline() {
        let c = (crate::arch::percpu::this_cpu().cpu_index as usize).min(CORES - 1);
        DEC[c].fetch_add(1, Relaxed);
    }

    /// INPUT-UNGATE: one relative-motion report summed into the producer's accumulator.
    #[inline]
    pub fn note_gui_coalesced() {
        GUI_COALESCED.fetch_add(1, Relaxed);
    }

    /// INPUT-UNGATE: one `try_send` refused by a full `GUI_CHANNEL_X86`.
    #[inline]
    pub fn note_gui_declined() {
        GUI_DECLINED.fetch_add(1, Relaxed);
    }

    /// INPUT-UNGATE: one `Event::Timer` heartbeat dropped rather than queued.
    #[inline]
    pub fn note_gui_timer_drop() {
        GUI_TIMER_DROP.fetch_add(1, Relaxed);
    }

    /// WCSER-REHOME: one singleton role re-homed off a core the steal declared dead.
    #[inline]
    pub fn note_rehome() {
        REHOMED.fetch_add(1, Relaxed);
    }

    /// True iff row `i` (0-based) has had NO attach since the last completed composite pass.
    /// Read by `wm`'s damage sampler; a pure pair of relaxed loads.
    #[inline]
    pub fn row_static(i: usize) -> bool {
        i < ROWS && ATT[i].load(Relaxed) == ATT_AT_PASS[i].load(Relaxed)
    }

    // ── the emit ───────────────────────────────────────────────────────────────────────────────

    /// Called from the APIC timer ISR on EVERY tick and every core. Returns immediately on all but
    /// the BSP's thousandth tick, so the steady-state cost is one relaxed load and one compare.
    #[inline]
    pub fn tick() {
        // The BSP is the only core that advances `APIC_TICKS`, so it is the only core whose clock
        // reading is the wall clock; letting an AP emit would produce lines on a different rate.
        if crate::arch::percpu::this_cpu().cpu_index != 0 {
            return;
        }
        let now = crate::arch::ms();
        if now < NEXT_DUE_MS.load(Relaxed) {
            return;
        }
        // Re-base off `now`, not off the old deadline: if the ISR was starved for several seconds
        // the instrument resumes at one line per second instead of emitting a burst of backlog.
        NEXT_DUE_MS.store(now.saturating_add(PERIOD_MS), Relaxed);
        emit(now);
    }

    /// Format and emit the line. Runs in the timer ISR with IF=0.
    ///
    /// `serial_println!` is safe from here, and that is a property of `arch::serial::_print`, not an
    /// assumption: it takes `SERIAL1` with `try_lock` ONLY (never `lock`), and on the rMBP — which
    /// has no 16550 at 0x3F8 — the `UART_STATE == 2` arm breaks out immediately without even
    /// entering the back-pressure loop. Its four downstream taps (`fbcon`, the FTDI console ring,
    /// the `tste` ring, the flight recorder) are each documented `try_lock`-only, alloc-free and
    /// safe from an IRQ-masked context. So this call acquires nothing it can wait on.
    fn emit(now: u64) {
        let hid = HID_IN.swap(0, Relaxed);
        let pmp = HID_POLL.swap(0, Relaxed);

        // Input queue depth, lock-free: `pal`'s own conservation law, `push - drop - pop`. The
        // blocking accessor `pal::event_queue_depth()` is deliberately NOT used — it is
        // `without_interrupts(|| EVENT_QUEUE.lock().len())` and would be a self-deadlock the day an
        // EHCI ISR exists. `saturating_sub` because the counter bumps and the head/tail move are not
        // one atomic step, so a concurrent push can be observed mid-flight and skew the sum by ±1.
        let (pp, pk, dp, dk, pop) = crate::pal::event_queue_stats();
        let hq = (pp + pk).saturating_sub(dp).saturating_sub(dk).saturating_sub(pop);

        let hid_ms = HID_LAST_MS.load(Relaxed);
        let comp_ms = COMP_LAST_MS.load(Relaxed);

        // `COMP_GATE`'s holder and hold age — a NON-BLOCKING read of two atoms `wm` already keeps as
        // gauges ("loaded, never drained", wm.rs:7750). The gate itself is never touched.
        let (holder, held) = crate::video::wm::deadman_gate_sample();

        let dmg = crate::video::wm::deadman_damage_sample();

        // One bounded stack buffer; no allocation in the ISR.
        let mut ln = Line::new();
        use core::fmt::Write;
        let _ = write!(ln, "[deadman] up={} hid={} pmp={} hq={}", now / 1000, hid, pmp, hq);
        // Ages, not stamps: a stamp forces the reader to do arithmetic against a clock that may
        // itself have stopped. `-` is the never-populated sentinel, never `0` masquerading as "0 ms
        // ago" — the distinction that makes "the screen never composited" readable.
        if hid_ms == NEVER {
            let _ = write!(ln, " hid_ms=-");
        } else {
            let _ = write!(ln, " hid_ms={}", now.saturating_sub(hid_ms));
        }
        if comp_ms == NEVER {
            let _ = write!(ln, " comp_ms=-");
        } else {
            let _ = write!(ln, " comp_ms={}", now.saturating_sub(comp_ms));
        }
        match holder {
            Some(c) => {
                let _ = write!(ln, " gate={}/{}", c, held);
            }
            None => {
                let _ = write!(ln, " gate=-");
            }
        }
        let _ = write!(ln, " dec=");
        for d in DEC.iter() {
            // Saturate at `f` rather than widening the field: a core that declined 15+ times in one
            // second is already "hammering the gate", and the exact count past that changes no
            // reading while a variable-width field would break a fixed `awk` column.
            let n = d.swap(0, Relaxed).min(0xF);
            let _ = write!(ln, "{:x}", n);
        }
        match dmg {
            Some((rows, stat)) => {
                let _ = write!(ln, " dmg={}/{}", rows, stat);
            }
            // TABLE was held by someone else at the instant we sampled. The instrument declines
            // rather than waits, and SAYS it declined — `?` is not `0`.
            None => {
                let _ = write!(ln, " dmg=?/?");
            }
        }
        // INPUT-UNGATE — `in=coalesced/declined/timerdrop` and `rh=`, the input path's own liveness,
        // drained per second like `hid`/`pmp` beside them.
        //
        // WHAT EACH READING MEANS, since a witness nobody can read is not one:
        //   `in=0/0/0`   the channel took everything offered. The normal desktop.
        //   `in=N/M/T` with `hq` FALLING — the render task is behind but alive; motion is being
        //              summed and heartbeats shed, which is exactly the designed degradation.
        //   `in=N/M/T` with `hq` PINNED and `rh=0` — the consumer is GONE and was not re-homed.
        //              This is boot 16's state, and it is the one this arc exists to make impossible.
        //   `rh>0`     a core the steal declared dead had its singleton roles reassigned. Pair it
        //              with `hq` returning to 0 to read the recovery as complete rather than merely
        //              attempted.
        // `rh` is NOT drained (a standing count), the other three ARE (a per-second rate) — so a
        // one-second burst of declines cannot be mistaken for a permanent one.
        let _ = write!(
            ln,
            " in={}/{}/{} rh={}",
            GUI_COALESCED.swap(0, Relaxed),
            GUI_DECLINED.swap(0, Relaxed),
            GUI_TIMER_DROP.swap(0, Relaxed),
            REHOMED.load(Relaxed),
        );
        serial_println!("{}", ln.as_str());
    }

    /// A bounded, allocation-free line buffer. `write_str` copies what fits and silently drops the
    /// rest, so a formatting surprise truncates the line instead of faulting in the timer ISR.
    /// Sized for the documented ~95-byte line with headroom for an 8-core `dec=` and wide counters.
    struct Line {
        b: [u8; 160],
        n: usize,
    }
    impl Line {
        fn new() -> Self {
            Self { b: [0; 160], n: 0 }
        }
        fn as_str(&self) -> &str {
            core::str::from_utf8(&self.b[..self.n]).unwrap_or("[deadman] <utf8-error>")
        }
    }
    impl core::fmt::Write for Line {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let room = self.b.len() - self.n;
            let take = core::cmp::min(room, s.len());
            self.b[self.n..self.n + take].copy_from_slice(&s.as_bytes()[..take]);
            self.n += take;
            Ok(())
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// SHIM — knob off, or off-arch. Every entry point is an empty inline function so that the call
// sites in `arch/x86_64/interrupts.rs`, `drivers/ehci/mod.rs` and `video/wm.rs` need no `#[cfg]`.
// ─────────────────────────────────────────────────────────────────────────────────────────────
#[cfg(not(all(feature = "deadman", target_arch = "x86_64")))]
mod imp {
    #[inline(always)]
    pub fn note_hid_completion() {}
    #[inline(always)]
    pub fn note_hid_poll() {}
    #[inline(always)]
    pub fn note_attach(_id: u32) {}
    #[inline(always)]
    pub fn note_composite_done() {}
    #[inline(always)]
    pub fn note_decline() {}
    #[inline(always)]
    pub fn note_gui_coalesced() {}
    #[inline(always)]
    pub fn note_gui_declined() {}
    #[inline(always)]
    pub fn note_gui_timer_drop() {}
    #[inline(always)]
    pub fn note_rehome() {}
    #[inline(always)]
    pub fn row_static(_i: usize) -> bool {
        false
    }
    #[inline(always)]
    pub fn tick() {}
}

pub use imp::*;
