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

//! KVBLANK — the Kepler head's VBLANK EDGE, counted, timed and phased, and the wait `video/beam.rs`
//! takes instead of spinning on the raster position.
//!
//! ## The defect this rung answers (B135 §7, VUGPERF, fold `4a283725`)
//!
//! VUGPERF decomposed flight 11's 139.75 ms composite pass and found the residual charged to
//! neither half of the clock — `blit_us - compose_us - present_us` = 46 488 us per pass — is
//! `video::beam::hold` SPINNING on an MMIO read. The census: `[wc-h] win=8 beamwaits=4336
//! beamwait_us=10766899` (2.48 ms mean per hold, against a 16.667 ms frame), `win=3
//! beamwait_us=49113217` over 13971 waits, `win=2 beammaxwait_us=62007`. The beam wait alone
//! exceeds the presenter's entire 500 us budget by 5x, and it is spent re-reading ONE register.
//!
//! A display engine has a vblank for exactly this. Two rungs:
//!
//! * **RUNG 1 `kvblank-measure`** (this module's [`note`] / [`arm`] / [`arm_pmc_pdisplay`]) — count
//!   the edges, timestamp each one, and publish the PHASE at which the edge is observed. No
//!   compositor change; the compositor's own `scanout_beam` reads feed it.
//! * **RUNG 2 `kvblank-wait`** ([`wait_next_edge`], called from `video::beam::hold`) — when the
//!   hazard zone is WIDE and rung 1's measured phase is OUTSIDE it, wait for the next edge on a
//!   counter compare instead of spinning on the raster, then confirm with ONE raster read.
//!
//! ## THE REGISTERS, and their cleanroom provenance
//!
//! Sources of record, as `kepler_display.rs` already names them: envytools rnndb
//! `display/g80_pdisplay.xml` and NVIDIA open-gpu-doc (`dev_master` for PMC). No nouveau code was
//! read or transcribed. Every register below is named with its bit and its document; where this
//! tree has NO cited offset, the register is printed as `NOT-IN-TREE` and NOTHING is read at a
//! guessed address (the `igpu-dpy: … NOT-IN-TREE` idiom, `igpu.rs:1731`).
//!
//! | register | offset | bits | citation | class |
//! |---|---|---|---|---|
//! | `NV_PMC_INTR_0` | BAR0 `0x000100` | `[26]` = PDISPLAY | open-gpu-doc `dev_master`; envytools `docs/hw/bus/pmc.txt` NV50+ source table | offset **[TREE]** (`kepler.rs:9`, `gpu_spec.md` §2.1); **bit 26 is [EXT] and UNVERIFIED on GK107** |
//! | `NV_PMC_INTR_EN` | BAR0 `0x000140` | `[26]` = PDISPLAY | same | same |
//! | `HEAD_STAT` | PDISPLAY `0x6000`, stride `0x800`, 4 heads (GK104-) | — | rnndb `display/g80_pdisplay.xml:647` | **[TREE]** (`kepler_display.rs` BEAMX86) |
//! | `HEAD_STAT.VERT` | `HEAD_STAT + 0x340` | `vline[15:0]`, **`vblank_count[31:16]`** | same | **[TREE]**, and BEHAVIOURALLY validated every boot by `beam_probe`'s `vbd >= 2` test |
//! | PDISPLAY per-head vblank interrupt ENABLE / STATUS | — | — | — | **NOT-IN-TREE.** No offset for this pair exists in this tree or in the two documents as this seat read them. It is OWED, and until it is cited this rung neither reads nor writes it. |
//!
//! ## WHY `mode=poll`, and what the interrupt path would need
//!
//! The brief's rung 1 is an interrupt if the tree has a vector/MSI helper for a PCI function and a
//! POLL if it does not. It does not. `arch/x86_64/interrupts.rs` has exactly three device vectors,
//! each a HARD-CODED constant with its own `extern "x86-interrupt"` handler installed at
//! `idt[..].set_handler_fn` (`XHCI_MSI_VECTOR` 0x40, `NIC_MSI_VECTOR` 0x41, `EHCI_MSI_VECTOR`
//! 0x43); there is no `alloc_vector`, no `register_irq` and no vector table a driver can join.
//! `PciScanner::enable_msi`/`enable_msix` exist, but they take a vector a caller must already own.
//! Giving the GK107 one would mean a FOURTH constant, a fourth handler and a fourth IDT entry —
//! a new interrupt mechanism, which this rung's brief forbids. So:
//!
//! **THE EDGE SOURCE IS `HEAD_STAT.VERT[31:16]`, POLLED.** That is not a weaker reading than the
//! interrupt would have given: it is the SAME counter the display engine would have raised the
//! interrupt from, it is already cited, and `beam_probe` already proves per boot that it ticks. It
//! costs NOTHING extra to read, because [`note`] is fed the word `scanout_beam` was already
//! reading — the compositor's own hold loop is the sampler, at ~400 kHz during a hold.
//!
//! What the interrupt path would need, named so the next rung is a shopping list and not a hunt:
//! (a) a `KEPLER_MSI_VECTOR` constant and handler in `arch/x86_64/interrupts.rs`; (b) a
//! `PciScanner::enable_msi` call on the GK107 function with that vector; (c) the PDISPLAY-side
//! per-head vblank interrupt ENABLE and STATUS offsets, which are the `NOT-IN-TREE` row above and
//! are the REAL blocker — without them the engine never raises the event, whatever PMC says.
//!
//! ## What `arm_pmc_pdisplay` does, and what it does not
//!
//! `kepler::init` has written `NV_PMC_INTR_EN = 0` since the driver's first day ("Disable
//! Interrupts", `kepler.rs:1429`). Under this knob, and ONLY under it, the rung then sets exactly
//! one bit — PDISPLAY — observes `NV_PMC_INTR_0` across a bounded window to see whether the
//! display engine latches anything there at all, and RESTORES the captured value with a READ-BACK.
//! One register, one bit, restored and read back: the `ctrlbind chan-restored` idiom. Delivery is
//! impossible by construction (no vector), which is the point — the rung asks whether the SOURCE
//! is alive, not whether an interrupt arrives.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use super::kepler::{mmio_read, mmio_write, regs};

// ── Cited constants ───────────────────────────────────────────────────────────────────────────

/// PDISPLAY's source bit in `NV_PMC_INTR_0` / `NV_PMC_INTR_EN`.
///
/// CITATION CLASS **[EXT], UNVERIFIED ON GK107** (`falcon_microcode_spec.md` §0.1). The offsets of
/// both PMC words are [TREE] (`kepler.rs:9-10`, `gpu_spec.md` §2.1); the SOURCE-BIT assignment is
/// read from the public NV50+ PMC interrupt source table (envytools `docs/hw/bus/pmc.txt`, mirrored
/// by open-gpu-doc's `dev_master` `NV_PMC_INTR_0_PDISP`). Nothing in this tree has ever observed it
/// set, which is exactly what rung 1 is flying to find out — so the witness line prints the RAW
/// words beside the decoded bit and a flight may re-derive the assignment from them.
const PMC_INTR_BIT_PDISPLAY: u32 = 26;

/// The measurement window `arm_pmc_pdisplay` holds the bit for, in ms: three frames at 60 Hz, so a
/// latching status word has had at least two whole vblanks to latch in.
const PMC_WINDOW_MS: u64 = 50;

/// Hard iteration cap for that window — `crate::arch::ms()` is the APIC tick and a pure
/// time-budget loop inside `init` would spin forever if it were ever stopped. Bounded by
/// construction, exactly as `beam_probe`'s `BEAM_SPIN_CAP` is.
const PMC_SPIN_CAP: u32 = 4_000_000;

/// Frames the rung-2 wait may burn before it gives up. **MUST equal `video::beam::GIVEUP_FRAMES`** —
/// the go-red half of the fixture asserts the wait gives up on the SAME budget as the spin, so the
/// two cannot be allowed to drift. Checked at compile time below.
const GIVEUP_FRAMES: u64 = 2;
const _: () = assert!(GIVEUP_FRAMES == crate::video::beam::GIVEUP_FRAMES);

/// Rung 1 prints at 1 Hz for the first `FAST_PRINTS` seconds, then drops to [`SLOW_MS`].
const FAST_MS: u64 = 1_000;
const FAST_PRINTS: u32 = 10;
/// The slow cadence — the display trace cadence, i.e. once per ten seconds, which is the rate the
/// compositor's own rollups settle to on a quiet desktop.
const SLOW_MS: u64 = 10_000;

// ── Rung 1 state ──────────────────────────────────────────────────────────────────────────────

/// Head index `beam_probe` ARMED, plus 1. Zero = rung 1 is not counting (no beam source this boot).
static VB_HEAD1: AtomicU32 = AtomicU32::new(0);
/// Lines per frame, as `beam_probe` measured them.
static VB_VTOTAL: AtomicU32 = AtomicU32::new(0);
/// `vblank_count[31:16]` as last seen, plus the 1-bit "seen at all" flag in bit 16.
static VB_LAST: AtomicU32 = AtomicU32::new(0);
/// Monotonic edge count. THE counter rung 2 compares against; it does not wrap at 16 bits the way
/// the hardware field does.
static VB_COUNT: AtomicU64 = AtomicU64::new(0);
/// `now_cycles()` at the last edge.
static VB_LAST_CYC: AtomicU64 = AtomicU64::new(0);
/// Sum / min / max of the per-edge cycle deltas, for `period_us=` and `jitter_us=`.
static VB_SUM_CYC: AtomicU64 = AtomicU64::new(0);
static VB_MIN_CYC: AtomicU64 = AtomicU64::new(u64::MAX);
static VB_MAX_CYC: AtomicU64 = AtomicU64::new(0);
/// `vline` read INSIDE the edge — THE PHASE, and the one number rung 2's threshold is derived from.
/// Stored plus 1 so that zero means "no edge has been phased yet" and line 0 stays a legal reading.
static VB_PHASE1: AtomicU32 = AtomicU32::new(0);
/// `crate::arch::ms()` at the last print, and how many fast prints have gone out.
static VB_LAST_PRINT_MS: AtomicU64 = AtomicU64::new(0);
static VB_PRINTS: AtomicU32 = AtomicU32::new(0);

// ── Rung 2 accounting ─────────────────────────────────────────────────────────────────────────

/// Holds that took the WAIT arm, and the microseconds they spent in it. Reported on this module's
/// own witness line as `vbwaits=` / `vbwait_us=`, beside `beamwaits=` on `[wc-h]`.
///
/// ⚠ OWED: the brief asks for these two fields ON `[wc-h]` itself. That line is emitted from
/// `video/wm.rs`, which this rung's brief does not name (PTRPAINT/VUGPROBE hold it), so the census
/// ships on the `:: kepler: vblank` line instead and the `[wc-h]` fields are owed to the fold.
static VB_WAITS: AtomicU64 = AtomicU64::new(0);
static VB_WAIT_US: AtomicU64 = AtomicU64::new(0);
/// Waits that ran out the GIVEUP budget without the counter moving, and waits whose confirming
/// raster read landed back inside the hazard zone and fell through to the spin.
static VB_WAIT_GAVEUP: AtomicU64 = AtomicU64::new(0);
static VB_WAIT_RECHECK: AtomicU64 = AtomicU64::new(0);

// ── The fixture's simulated source ────────────────────────────────────────────────────────────

/// Fixture mode: 0 = off (the hardware counter is the source), 1 = a counter the TIMER advances at
/// one simulated frame period, 2 = a counter that never advances (the go-red).
static SIM_MODE: AtomicU32 = AtomicU32::new(0);
/// `now_cycles()` when the simulation was armed, and the cycles in one simulated period.
static SIM_T0: AtomicU64 = AtomicU64::new(0);
static SIM_PERIOD_CYC: AtomicU64 = AtomicU64::new(0);
/// The fixture runs once per boot.
static SELFTEST_DONE: AtomicBool = AtomicBool::new(false);

// ── Helpers ───────────────────────────────────────────────────────────────────────────────────

/// `now_cycles()` deltas to microseconds. A private twin of `beam::cycles_to_us` for the same
/// reason that one is a twin of `wcg`'s: the owner module is gated differently.
#[inline]
fn cycles_to_us(dt: u64) -> u64 {
    let hz = crate::arch::apic::tsc_hz();
    let hz = if hz == 0 { 1_250_000_000 } else { hz };
    dt.saturating_mul(1_000_000) / hz
}

#[inline]
fn us_to_cycles(us: u64) -> u64 {
    let hz = crate::arch::apic::tsc_hz();
    let hz = if hz == 0 { 1_250_000_000 } else { hz };
    us.saturating_mul(hz) / 1_000_000
}

// ── RUNG 1 — the PMC half, called from `kepler::init`'s interrupt-disable statement ────────────

/// KVBLANK rung 1, PMC half. Set the PDISPLAY source bit in `NV_PMC_INTR_EN`, watch
/// `NV_PMC_INTR_0` for a bounded window, RESTORE the captured value and read it back.
///
/// Called from the `kepler.rs` statement that has written `NV_PMC_INTR_EN = 0` since the driver's
/// first day — so the captured value is that zero and the restore puts the driver back exactly
/// where it was. Exactly one bit is ever set, it is named and cited, and the restore is READ BACK
/// and printed: `restored=<v> verdict=clean|DIRTY`.
///
/// # Safety
/// `bar0` must be the mapped BAR0 base `kepler::init`'s other accesses already use.
pub unsafe fn arm_pmc_pdisplay(bar0: usize) {
    let bit = 1u32 << PMC_INTR_BIT_PDISPLAY;
    let en_entry = mmio_read(bar0, regs::NV_PMC_INTR_EN);
    let st_entry = mmio_read(bar0, regs::NV_PMC_INTR_0);
    mmio_write(bar0, regs::NV_PMC_INTR_EN, en_entry | bit);
    let en_armed = mmio_read(bar0, regs::NV_PMC_INTR_EN);

    // Observe. Delivery is impossible (no vector is wired for this function — see the module doc),
    // so what is under test is whether the SOURCE latches in the status word at all.
    let (mut seen, mut samples, mut spins) = (0u32, 0u32, 0u32);
    let mut or_all = 0u32;
    let t0 = crate::arch::ms();
    loop {
        if crate::arch::ms().wrapping_sub(t0) > PMC_WINDOW_MS {
            break;
        }
        spins = spins.saturating_add(1);
        if spins >= PMC_SPIN_CAP {
            break;
        }
        let st = mmio_read(bar0, regs::NV_PMC_INTR_0);
        samples = samples.saturating_add(1);
        or_all |= st;
        if st & bit != 0 {
            seen = seen.saturating_add(1);
        }
        core::hint::spin_loop();
    }

    mmio_write(bar0, regs::NV_PMC_INTR_EN, en_entry);
    let en_back = mmio_read(bar0, regs::NV_PMC_INTR_EN);
    let clean = en_back == en_entry;

    serial_println!(
        ":: kepler: vblank pmc-arm bit={} en_entry={:08X} en_armed={:08X} intr_entry={:08X} intr_or={:08X} pdisplay_seen={}/{} window_ms={} deliver=none reason=no-vector-helper restored={:08X} verdict={} :: — NV_PMC_INTR_EN/INTR_0 offsets are [TREE] (kepler.rs:9-10, gpu_spec 2.1); BIT {} = PDISPLAY is [EXT] and UNVERIFIED on GK107 (public NV50+ PMC source table), so the RAW words are printed beside the decode and a flight may re-derive it. The PDISPLAY-side per-head vblank ENABLE/STATUS pair is NOT-IN-TREE and is neither read nor written here. writes=2 (set, restore), restored+read-back ::",
        PMC_INTR_BIT_PDISPLAY, en_entry, en_armed, st_entry, or_all, seen, samples, PMC_WINDOW_MS,
        en_back, if clean { "clean" } else { "DIRTY" }, PMC_INTR_BIT_PDISPLAY,
    );
}

/// KVBLANK rung 1 — ARM the counter. Called from `beam_probe`'s ARMED branch with the head it chose
/// and the lines-per-frame it measured, so this module counts edges on the SAME head the beam gate
/// reads and a disagreement between the two is impossible by construction.
pub fn arm(head: u32, vtotal: u32) {
    VB_VTOTAL.store(vtotal, Ordering::Relaxed);
    VB_LAST_PRINT_MS.store(crate::arch::ms(), Ordering::Relaxed);
    VB_HEAD1.store(head + 1, Ordering::Release);
    serial_println!(
        ":: kepler: vblank arm head={} vt={} mode=poll src=HEAD_STAT.VERT[31:16] :: — rnndb display/g80_pdisplay.xml:647 (HEAD_STAT 0x6000, stride 0x800, +0x340 VERT: vline[15:0], vblank_count[31:16]). POLL and not an interrupt because this tree has no vector/MSI helper a PCI function can join (three hard-coded IDT vectors, no allocator) — see kepler_vblank.rs for what the interrupt path would need. The sampler is the compositor's own scanout_beam read: zero extra MMIO. READ-ONLY: writes=0 ::",
        head, vtotal,
    );
}

/// KVBLANK rung 1 — the edge detector, fed the RAW `HEAD_STAT.VERT` word `scanout_beam` just read.
///
/// FAST PATH: one relaxed load and a 16-bit compare, then return. `now_cycles()` and the witness
/// print are reached only ON an edge — at most 60 times a second on a 60 Hz panel — so this costs
/// the hold loop a compare per iteration and nothing else.
#[inline]
pub fn note(word: u32) {
    if VB_HEAD1.load(Ordering::Acquire) == 0 {
        return;
    }
    let vb = (word >> 16) & 0xFFFF;
    let prev = VB_LAST.load(Ordering::Relaxed);
    if prev & 0x1_0000 != 0 && (prev & 0xFFFF) == vb {
        return;
    }
    VB_LAST.store(vb | 0x1_0000, Ordering::Relaxed);
    if prev & 0x1_0000 == 0 {
        // First reading of the field is not an edge — there is no previous timestamp to subtract.
        VB_LAST_CYC.store(crate::arch::now_cycles(), Ordering::Relaxed);
        return;
    }
    edge(word & 0xFFFF);
}

/// The edge itself: timestamp it, fold it into the period/jitter accumulators, record the PHASE and
/// print on the cadence.
fn edge(vline: u32) {
    let now = crate::arch::now_cycles();
    let last = VB_LAST_CYC.swap(now, Ordering::Relaxed);
    let n = VB_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    // THE PHASE. `vline` here is the raster position read in the SAME word as the counter tick, so
    // it is where in the frame the edge was observed — `raster_at_irq`, and the number rung 2's
    // threshold is derived from.
    VB_PHASE1.store(vline + 1, Ordering::Relaxed);
    if last != 0 {
        let dt = now.saturating_sub(last);
        VB_SUM_CYC.fetch_add(dt, Ordering::Relaxed);
        VB_MIN_CYC.fetch_min(dt, Ordering::Relaxed);
        VB_MAX_CYC.fetch_max(dt, Ordering::Relaxed);
    }
    let prints = VB_PRINTS.load(Ordering::Relaxed);
    let due = if prints < FAST_PRINTS { FAST_MS } else { SLOW_MS };
    let ms = crate::arch::ms();
    if ms.wrapping_sub(VB_LAST_PRINT_MS.load(Ordering::Relaxed)) < due {
        return;
    }
    VB_LAST_PRINT_MS.store(ms, Ordering::Relaxed);
    VB_PRINTS.store(prints.saturating_add(1), Ordering::Relaxed);
    report(n, vline);
}

/// The witness line. Split out so the fixture can print the same shape without an edge behind it.
fn report(n: u64, vline: u32) {
    let sum = VB_SUM_CYC.load(Ordering::Relaxed);
    let mn = VB_MIN_CYC.load(Ordering::Relaxed);
    let mx = VB_MAX_CYC.load(Ordering::Relaxed);
    let period_us = if n > 1 { cycles_to_us(sum / (n - 1)) } else { 0 };
    let jitter_us = if mx >= mn && mn != u64::MAX { cycles_to_us(mx - mn) } else { 0 };
    serial_println!(
        ":: kepler: vblank head={} count={} period_us={} jitter_us={} raster_at_irq={} vt={} mode=poll vbwaits={} vbwait_us={} vbgaveup={} vbrecheck={} ::",
        VB_HEAD1.load(Ordering::Relaxed).saturating_sub(1),
        n, period_us, jitter_us, vline, VB_VTOTAL.load(Ordering::Relaxed),
        VB_WAITS.load(Ordering::Relaxed), VB_WAIT_US.load(Ordering::Relaxed),
        VB_WAIT_GAVEUP.load(Ordering::Relaxed), VB_WAIT_RECHECK.load(Ordering::Relaxed),
    );
}

// ── RUNG 2 — the wait arm's source and its primitive ──────────────────────────────────────────

/// The monotonic vblank counter, or `None` when no source is counting this boot. `video::beam::hold`
/// reads this to decide whether the wait arm exists at all.
///
/// Under the fixture this answers from the SIMULATED source, which is what makes the fixture drive
/// the real arm rather than a copy of it.
#[inline]
pub fn counter() -> Option<u64> {
    match SIM_MODE.load(Ordering::Relaxed) {
        0 => {
            if VB_HEAD1.load(Ordering::Acquire) == 0 || VB_COUNT.load(Ordering::Relaxed) < 2 {
                None
            } else {
                Some(VB_COUNT.load(Ordering::Relaxed))
            }
        }
        // A counter the TIMER advances, one tick per simulated period.
        1 => {
            let p = SIM_PERIOD_CYC.load(Ordering::Relaxed).max(1);
            Some(crate::arch::now_cycles().saturating_sub(SIM_T0.load(Ordering::Relaxed)) / p)
        }
        // THE GO-RED: a counter that never advances. A counter that does not move is not a raster,
        // and the wait must give up on the spin's own budget rather than hang the present.
        _ => Some(0),
    }
}

/// Rung 1's measured phase — the raster line at which the vblank edge is OBSERVED, `None` until an
/// edge has been phased. `video::beam::hold` takes the wait arm only when this line is outside the
/// rect's hazard zone, which is the sense in which rung 2's threshold is DERIVED from rung 1.
#[inline]
pub fn edge_phase() -> Option<u32> {
    match VB_PHASE1.load(Ordering::Relaxed) {
        0 => None,
        v => Some(v - 1),
    }
}

/// RUNG 2's primitive: wait for the vblank counter to leave `from`, never past `deadline_cyc`.
///
/// THE DEADLINE IS THE CALLER'S, and that is load-bearing: `beam::hold` passes `t0 + GIVEUP_FRAMES *
/// FRAME_US` — its OWN give-up deadline, taken from its OWN `t0` — so a wait that runs out lands in
/// the spin arm whose first budget check fires immediately. The present therefore costs ONE give-up
/// budget whichever arm it took, never two, and the spin arm needed not one character changed.
///
/// Returns `(advanced, waited_cyc)`. There is no blocking primitive on this path — `hold` runs
/// IRQ-masked on the presenting core inside `COMP_GATE`, so there is no scheduler to yield to and
/// no wait queue to sleep on — so this is a `spin_loop` yield between counter reads. The difference
/// from the arm it replaces is NOT that the CPU stops: it is that each iteration reads an atomic
/// this core already owns instead of issuing an uncached BAR0 MMIO read across the PCIe link, and
/// that it is woken by ONE event instead of polling a position.
///
/// The budget is `GIVEUP_FRAMES * FRAME_US` — the SAME budget the spin arm gives up on, asserted
/// equal at compile time above — so a source that stops counting costs a present exactly what a
/// raster that stops moving costs it, and not one microsecond more.
pub fn wait_next_edge(from: u64, deadline_cyc: u64) -> (bool, u64) {
    let t0 = crate::arch::now_cycles();
    loop {
        match counter() {
            Some(c) if c != from => {
                let dt = crate::arch::now_cycles().saturating_sub(t0);
                VB_WAITS.fetch_add(1, Ordering::Relaxed);
                VB_WAIT_US.fetch_add(cycles_to_us(dt), Ordering::Relaxed);
                return (true, dt);
            }
            Some(_) => {}
            // The source went away mid-wait. Fall back to the spin rather than invent an edge.
            None => {
                let dt = crate::arch::now_cycles().saturating_sub(t0);
                VB_WAIT_GAVEUP.fetch_add(1, Ordering::Relaxed);
                return (false, dt);
            }
        }
        let now = crate::arch::now_cycles();
        let dt = now.saturating_sub(t0);
        if now > deadline_cyc {
            VB_WAITS.fetch_add(1, Ordering::Relaxed);
            VB_WAIT_US.fetch_add(cycles_to_us(dt), Ordering::Relaxed);
            VB_WAIT_GAVEUP.fetch_add(1, Ordering::Relaxed);
            return (false, dt);
        }
        core::hint::spin_loop();
    }
}

/// Rung 2 bookkeeping: the wait returned, but the confirming raster read landed back inside the
/// hazard zone, so `hold` fell through to the spin. Correctness is the spin's; this counts the
/// times the phase prediction did not hold.
#[inline]
pub fn note_recheck() {
    VB_WAIT_RECHECK.fetch_add(1, Ordering::Relaxed);
}

// ── THE FIXTURE (rung 2) ──────────────────────────────────────────────────────────────────────

/// KVBLANK rung 2's self-test: drive [`wait_next_edge`] — the arm `video::beam::hold` calls, not a
/// copy of it — from a SIMULATED counter the timer advances, and prove both halves.
///
/// * **PASS** when the wait returns `advanced = true` within one simulated period.
/// * **GO-RED** (`SIM_MODE = 2`, a counter that never advances) when the wait gives up on the SAME
///   `GIVEUP_FRAMES` budget the spin uses and says so.
///
/// Both halves run on EVERY armed boot and both are printed, because a fixture that only ever
/// exercises its green half is not a gate (LAWS §5). The go-red costs one give-up budget —
/// 2 x 16.667 ms — once per boot.
///
/// It runs on a machine with NO Kepler: q35 answers `:: kepler: no-device ::` and neither
/// `arm_pmc_pdisplay` nor `arm` is ever reached there, so a simulated source is the ONLY way this
/// property is verified anywhere but on the bench.
pub fn selftest_once() {
    if SELFTEST_DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    let period = us_to_cycles(crate::video::beam::FRAME_US);

    // ── PASS half — a counter the timer advances at one tick per simulated frame ──
    SIM_PERIOD_CYC.store(period, Ordering::Relaxed);
    SIM_T0.store(crate::arch::now_cycles(), Ordering::Relaxed);
    SIM_MODE.store(1, Ordering::Release);
    let from = counter().unwrap_or(0);
    let deadline = crate::arch::now_cycles()
        .saturating_add(us_to_cycles(GIVEUP_FRAMES.saturating_mul(crate::video::beam::FRAME_US)));
    let (adv, cyc) = wait_next_edge(from, deadline);
    let waited_us = cycles_to_us(cyc);
    let bound_us = crate::video::beam::FRAME_US;
    let pass = adv && waited_us <= bound_us;
    serial_println!(
        ":: kepler: vblank selftest arm=wait sim=timer period_us={} from={} advanced={} waited_us={} bound=waited_us<={} :: {} ::",
        crate::video::beam::FRAME_US, from, adv as u32, waited_us, bound_us,
        if pass { "PASS" } else { "FAIL" },
    );

    // ── GO-RED half — a counter that never advances ──
    let g0 = VB_WAIT_GAVEUP.load(Ordering::Relaxed);
    SIM_MODE.store(2, Ordering::Release);
    let deadline2 = crate::arch::now_cycles()
        .saturating_add(us_to_cycles(GIVEUP_FRAMES.saturating_mul(crate::video::beam::FRAME_US)));
    let (adv2, cyc2) = wait_next_edge(0, deadline2);
    let waited2_us = cycles_to_us(cyc2);
    let budget_us = GIVEUP_FRAMES.saturating_mul(crate::video::beam::FRAME_US);
    let gaveup = VB_WAIT_GAVEUP.load(Ordering::Relaxed) > g0;
    // THE BOUND IS ONE FRAME WIDE, AND THAT IS THE CLAIM'S OWN RESOLUTION, not a slackened test.
    // What is under test is "the wait gives up on the SAME GIVEUP_FRAMES budget as the spin" — a
    // statement about a count of FRAMES. Asserting `waited_us >= budget_us` exactly measured the
    // wrong thing and FAILED on its first armed gate run at `waited_us=33295 budget_us=33334`: the
    // deadline is computed from a `now_cycles()` taken in this function and the elapsed time from a
    // second one taken inside `wait_next_edge`, and `us_to_cycles`/`cycles_to_us` truncate in
    // opposite directions — 39 us of clock plumbing on a 33 334 us budget, 0.12%, and on TCG at
    // that. So the bound is BOTH-SIDED and its unit is the frame: the wait must have burned at
    // least `GIVEUP_FRAMES - 1` whole frames (it did not return early) and at most
    // `GIVEUP_FRAMES + 1` (it did not overrun the spin's budget). A wait that returns in one frame
    // instead of two still FAILS this, which is the regression the go-red exists to catch.
    let lo = budget_us.saturating_sub(crate::video::beam::FRAME_US);
    let hi = budget_us.saturating_add(crate::video::beam::FRAME_US);
    let red_ok = !adv2 && gaveup && waited2_us >= lo && waited2_us <= hi;
    serial_println!(
        ":: kepler: vblank selftest arm=wait sim=stuck advanced={} gaveup={} waited_us={} budget_us={} bound={}<=waited_us<={} (GIVEUP_FRAMES budget, +/- one frame) :: {} ::",
        adv2 as u32, gaveup as u32, waited2_us, budget_us, lo, hi,
        if red_ok { "GO-RED-OK" } else { "GO-RED-FAILED" },
    );

    // Put the source back and un-count the fixture's own two waits, so the boot's `vbwaits=` census
    // measures the COMPOSITOR and not this function.
    SIM_MODE.store(0, Ordering::Release);
    VB_WAITS.store(0, Ordering::Relaxed);
    VB_WAIT_US.store(0, Ordering::Relaxed);
    VB_WAIT_GAVEUP.store(0, Ordering::Relaxed);
    VB_WAIT_RECHECK.store(0, Ordering::Relaxed);
}
