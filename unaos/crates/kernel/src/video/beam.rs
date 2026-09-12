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

//! BEAM — panel presents ordered against the scan-out's raster position, and the tear
//! instrument that OBSERVES a beam crossing instead of inferring one from a duration.
//!
//! ## The defect this closes (orin 24, `docs/dev/evidence/orin24/TEAR-DIAG.md`)
//!
//! Every present on the Orin writes rows `[y0, y1)` of the firmware's single live scan-out —
//! no back buffer, no flip, no vblank reference — so the write lands at a uniformly random phase
//! of the frame and the panel shows a seam whenever the beam is crossing those rows while they
//! change: `P(tear) = min(1, (present_us + rectscan_us) / FRAME_US)`, 0.7 for a 780-row window
//! however fast the copy is. The four `torn=` counters asked `present_us > rectscan_us` — a
//! DURATION question, anti-monotone in rect height — and read 0 across three boots while Peter
//! saw "A LOT OF TEARING". A duration cannot see a phase. Only the beam's position can.
//!
//! ## The mechanism
//!
//! [`crate::arch::scanout_beam`] answers `Some((vline, vtotal))` when the platform can read the
//! raster generator's current line — the Orin's nvdisplay `RG_DPCA`, locked by
//! `display_tegra::beam_probe` at boot under the JD1-DC power guard — and `None` everywhere else
//! (x86, Pi, QEMU: no source is wired, and every entry point below folds to a no-op there, so those
//! platforms' panel writes are byte-for-byte what they were).
//!
//! With a source, a present is BRACKETED. [`hold`] spins until the beam is outside the rect's
//! HAZARD ZONE — the rect plus a lead that covers the write (row copies AND the cache clean that
//! publishes them to a non-snooping scan-out) and the display's fetch-ahead — then the caller
//! copies its rows and cleans them INSIDE the bracket; [`settle`] samples the beam again and
//! records whether its forward path from the first sample to the second crossed the rect. That
//! record is the observation `wcg` prints as `torn=`: it goes non-zero on a torn frame because it
//! was there when the frame tore, and it reads 0 under the hold because the hold is what makes it
//! so. A present whose write outran its lead (a stall) is caught by the same test.
//!
//! ## Geometry, in lines modulo `vtotal`
//!
//! ```text
//! hazard zone   Z  = [y0 - LEAD - FETCH, y1)
//! LEAD             = span/3 + 16 for a staged present (the Orin copies 780 rows in ~1.1 ms and
//!                    cleans them in the same order of time against the beam's 10.8 ms on those
//!                    rows: a 3-4x ratio, and /3 is its conservative side);
//!                    span/2 + 16 for the direct path (per-pixel pokes, ~5 us/row measured)
//! FETCH            = 64 lines. nvdisplay fetches ahead of the raster through its mempool; the
//!                    depth is not published, and 64 lines (~0.9 ms) is the generous bound.
//! torn             = the beam's forward path vs -> ve intersects [y0 - FETCH, y1), or the
//!                    bracket lasted a whole frame (the path wrapped: P = 1).
//! give-up          = the beam sat in Z for two frames. A counter that does not move is not a
//!                    raster; the present proceeds unheld and is counted `gaveup`.
//! ```
//!
//! The wait is bounded by `rectscan_us + lead` — under one frame — and it runs where the present
//! already runs: IRQ-masked, on the presenting core, inside `COMP_GATE`. That is the price of a
//! tear-free present on a single buffer and it is the same price a vsync'd flip pays; the
//! difference is only that the wait is on the CPU instead of in the display engine, because the
//! display engine on this path is the firmware's and this kernel does not own a channel on it.
//!
//! ## What is recorded, and who reads it
//!
//! The bracket runs on non-witness builds too (it is the FIX, not the instrument); the observation
//! it produces is parked per core in [`LAST`] and taken by the witness that reports the present —
//! `wcg::stage_note` for windows, `wcg::erase_note` for desktop fills, `strip::bar_painted` for
//! the furniture — each on the same core, inside the same IRQ-masked pass, before any other
//! bracket on that core can run. A present that reaches the panel with no instrument behind it
//! (the direct fallback, the furniture's own vacate erase) holds with `record = false` and hands
//! the observation to its caller instead, so a slot is never left holding a stranger's verdict.

// KNOB-OFF FOLD (orin 27): every item below that names an atomic, a clock or the raster
// geometry is `beam`-gated, and `hold`/`settle`/`take_last` have `#[inline(always)]` constant
// twins for the OFF polarity. `video::beam` is declared unconditionally (it is the mechanism, not
// an instrument) but off-knob it must COMPILE TO NOTHING: `arch::scanout_beam()` being a constant
// `None` only folds the bodies if the call itself folds, and `hold` is too large for LLVM to
// inline on its own. Measured by `./arroyo knoboff beam` (EXECUTOR-BRIEF §5).
#[cfg(feature = "beam")]
use core::sync::atomic::{AtomicU64, Ordering};

/// One frame period at 60 Hz. The same figure `wcg` computes `rectscan_us` from, and the unit
/// `exposure_ppk` is measured in.
pub const FRAME_US: u64 = 16_667;

/// Lines the scan-out is assumed to have fetched ahead of the raster position it reports.
#[cfg(feature = "beam")]
const FETCH_LINES: u32 = 64;

/// The floor under the write lead, in lines: sampling latency and the MMIO round trip.
#[cfg(feature = "beam")]
const LEAD_MIN: u32 = 16;

/// Frames the hold may spin before it gives up on a counter that is not moving.
#[cfg(feature = "beam")]
const GIVEUP_FRAMES: u64 = 2;

/// Per-core observation slots. Eight covers every part this kernel runs on; a higher core index
/// folds onto the last slot, exactly as `wm::stage_pool_index` folds its stage buffers.
#[cfg(feature = "beam")]
const SLOTS: usize = 8;

/// An open bracket: the geometry the hold was taken over and the beam as it was released.
/// Off-knob nothing constructs one (`hold` is the constant `None` twin), so its fields are dead.
#[cfg_attr(not(feature = "beam"), allow(dead_code))]
pub struct Hold {
    y0: u32,
    y1: u32,
    vt: u32,
    vs: u32,
    waited_cyc: u64,
    gaveup: bool,
    record: bool,
    t_open: u64,
}

/// One present's beam observation, as the witnesses read it. Off-knob `take_last` never yields
/// one, so a witness build with `beam` off reads none of these fields.
#[cfg_attr(not(feature = "beam"), allow(dead_code))]
#[derive(Clone, Copy)]
pub struct Obs {
    /// The raster line when the first byte was written (after the hold).
    pub vs: u32,
    /// The raster line after the last byte was cleaned.
    pub ve: u32,
    /// Lines per frame, as the probe measured it (active rows plus blanking).
    pub vt: u32,
    /// Microseconds the hold spun, summed over the bands of one present.
    pub waited_us: u32,
    /// The beam crossed the rows while they were being written. THE tear verdict.
    pub torn: bool,
    /// The hold ran out its budget and let the present through unheld.
    pub gaveup: bool,
}

/// Packed per-core observation: bits 0..16 `vs`, 16..32 `ve`, 32..48 `vt`, 48 valid, 49 torn,
/// 50 gaveup. Zero is "nothing recorded".
#[cfg(feature = "beam")]
static LAST: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
/// Per-core microseconds waited since the slot was last taken.
#[cfg(feature = "beam")]
static LAST_WAIT: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];

#[cfg(feature = "beam")]
const F_VALID: u64 = 1 << 48;
#[cfg(feature = "beam")]
const F_TORN: u64 = 1 << 49;
#[cfg(feature = "beam")]
const F_GAVEUP: u64 = 1 << 50;

#[cfg(feature = "beam")]
#[inline]
fn slot() -> usize {
    crate::arch::sched::meter_current_cpu().min(SLOTS - 1)
}

/// `now_cycles()` deltas to microseconds. A private twin of `wcg::cycles_to_us` because that
/// module is witness-only and this one is not.
#[cfg(feature = "beam")]
#[cfg(target_arch = "aarch64")]
#[inline]
fn cycles_to_us(dt: u64) -> u64 {
    let frq = crate::arch::timer::cntfrq();
    let frq = if frq == 0 { 54_000_000 } else { frq };
    dt.saturating_mul(1_000_000) / frq
}

#[cfg(feature = "beam")]
#[cfg(target_arch = "x86_64")]
#[inline]
fn cycles_to_us(dt: u64) -> u64 {
    let hz = crate::arch::apic::tsc_hz();
    let hz = if hz == 0 { 1_250_000_000 } else { hz };
    dt.saturating_mul(1_000_000) / hz
}

#[cfg(feature = "beam")]
#[cfg(target_arch = "aarch64")]
#[inline]
fn us_to_cycles(us: u64) -> u64 {
    let frq = crate::arch::timer::cntfrq();
    let frq = if frq == 0 { 54_000_000 } else { frq };
    us.saturating_mul(frq) / 1_000_000
}

#[cfg(feature = "beam")]
#[cfg(target_arch = "x86_64")]
#[inline]
fn us_to_cycles(us: u64) -> u64 {
    let hz = crate::arch::apic::tsc_hz();
    let hz = if hz == 0 { 1_250_000_000 } else { hz };
    us.saturating_mul(hz) / 1_000_000
}

/// Half-open interval test on the frame's circle: is `v` in `[a, b)` modulo `vt`, where `a` and
/// `b` are already reduced and `a > b` means the interval wraps through 0.
#[cfg(feature = "beam")]
#[inline]
fn in_zone(v: u32, a: u32, b: u32) -> bool {
    if a <= b {
        v >= a && v < b
    } else {
        v >= a || v < b
    }
}

/// Open a bracket over panel rows `[y0, y1)` of a `panel_h`-row panel: spin until the beam is
/// clear of the hazard zone, then return the open bracket for [`settle`]. `slow` selects the
/// direct-path lead (see the module doc); `record` says whether [`settle`] parks the observation
/// for the witness on this core, or only returns it.
///
/// `None` means there is no beam source on this platform (or no rows): nothing waited, nothing
/// will be recorded, and the caller's present runs exactly as it did before this module.
#[cfg(feature = "beam")]
pub fn hold(y0: usize, y1: usize, panel_h: usize, slow: bool, record: bool) -> Option<Hold> {
    let (v0, vt) = crate::arch::scanout_beam()?;
    if vt == 0 || panel_h == 0 {
        return None;
    }
    let y0 = y0.min(panel_h) as u32;
    let y1 = y1.min(panel_h) as u32;
    if y1 <= y0 {
        return None;
    }
    let span = y1 - y0;
    let lead = if slow { span / 2 } else { span / 3 } + LEAD_MIN + FETCH_LINES;
    // `y0 - lead` on the circle: add `vt` first so the subtraction cannot underflow.
    let a = (y0 + vt - (lead % vt)) % vt;
    let b = y1 % vt;
    let t0 = crate::arch::now_cycles();
    let budget = us_to_cycles(GIVEUP_FRAMES * FRAME_US);
    let mut v = v0.min(vt - 1);
    let mut gaveup = false;
    while in_zone(v, a, b) {
        if crate::arch::now_cycles().saturating_sub(t0) > budget {
            gaveup = true;
            break;
        }
        core::hint::spin_loop();
        match crate::arch::scanout_beam() {
            Some((nv, _)) => v = nv.min(vt - 1),
            None => {
                gaveup = true;
                break;
            }
        }
    }
    let t1 = crate::arch::now_cycles();
    Some(Hold {
        y0,
        y1,
        vt,
        vs: v,
        waited_cyc: t1.saturating_sub(t0),
        gaveup,
        record,
        t_open: t1,
    })
}

/// Whether a bracket is open, i.e. a beam source exists and the caller must publish (cache-clean)
/// its rows INSIDE the bracket for the observation to mean anything.
#[inline]
pub fn held(h: &Option<Hold>) -> bool {
    h.is_some()
}

/// Cycles the hold spun, so a caller can take them back out of a clock it opened before the hold.
#[inline]
pub fn waited_cycles(h: &Option<Hold>) -> u64 {
    match h {
        Some(h) => h.waited_cyc,
        None => 0,
    }
}

/// Close a bracket: sample the beam, decide whether it crossed the rows while they were being
/// written, park the observation for this core's witness (when `record`), and return it.
#[cfg(feature = "beam")]
pub fn settle(h: Option<Hold>) -> Option<Obs> {
    let h = h?;
    let now = crate::arch::now_cycles();
    let ve = match crate::arch::scanout_beam() {
        Some((v, _)) => v.min(h.vt - 1),
        None => h.vs,
    };
    // The zone the OBSERVATION is judged against carries the fetch-ahead only: the write lead was
    // the hold's margin, not a claim about where the beam may be.
    let za = (h.y0 + h.vt - (FETCH_LINES % h.vt)) % h.vt;
    let zb = h.y1 % h.vt;
    let path = (ve + h.vt - h.vs) % h.vt;
    let to_zone = (za + h.vt - h.vs) % h.vt;
    let wrapped = cycles_to_us(now.saturating_sub(h.t_open)) >= FRAME_US;
    let torn = wrapped || in_zone(h.vs, za, zb) || in_zone(ve, za, zb) || to_zone <= path;
    let waited_us = cycles_to_us(h.waited_cyc).min(u32::MAX as u64) as u32;
    let obs = Obs { vs: h.vs, ve, vt: h.vt, waited_us, torn, gaveup: h.gaveup };
    if h.record {
        let i = slot();
        // Accumulate across the bands of one present: keep the first band's `vs`, take this
        // band's `ve`, OR the flags. A slot with nothing in it takes everything.
        let prev = LAST[i].load(Ordering::Relaxed);
        let vs = if prev & F_VALID != 0 { prev & 0xFFFF } else { h.vs as u64 };
        let mut w = vs | ((ve as u64) << 16) | ((h.vt as u64) << 32) | F_VALID | (prev & (F_TORN | F_GAVEUP));
        if torn {
            w |= F_TORN;
        }
        if h.gaveup {
            w |= F_GAVEUP;
        }
        LAST[i].store(w, Ordering::Relaxed);
        LAST_WAIT[i].fetch_add(waited_us as u64, Ordering::Relaxed);
    }
    Some(obs)
}

/// Take (and clear) the observation parked for this core, if a recorded bracket closed since the
/// last take. Called by the witness that reports the present, on the same core, in the same pass.
#[cfg(feature = "beam")]
pub fn take_last() -> Option<Obs> {
    let i = slot();
    let w = LAST[i].swap(0, Ordering::Relaxed);
    let waited = LAST_WAIT[i].swap(0, Ordering::Relaxed);
    if w & F_VALID == 0 {
        return None;
    }
    Some(Obs {
        vs: (w & 0xFFFF) as u32,
        ve: ((w >> 16) & 0xFFFF) as u32,
        vt: ((w >> 32) & 0xFFFF) as u32,
        waited_us: waited.min(u32::MAX as u64) as u32,
        torn: w & F_TORN != 0,
        gaveup: w & F_GAVEUP != 0,
    })
}

/// The beam-crossing EXPOSURE of one present, in per-mille: `min(1, (present_us + rectscan_us) /
/// FRAME_US)` — the probability that a present of this duration over rows the beam takes
/// `rectscan_us` to cross tore, at a uniformly random phase. Summed per window it is the expected
/// torn-present count, which is what a BLIND build (no beam source) can still say about a boot;
/// the duration predicate it sits beside cannot. TEAR-DIAG §2.
#[inline]
pub fn exposure_ppk(present_us: u64, rectscan_us: u64) -> u64 {
    (present_us.saturating_add(rectscan_us).saturating_mul(1000) / FRAME_US).min(1000)
}

// ── KNOB-OFF TWINS (orin 27) ───────────────────────────────────────────────────────────────────
//
// With `beam` off there is no raster source anywhere in the tree — `arch::scanout_beam()` is a
// constant `None` on both arches — so the three entry points above are constants too, and
// `#[inline(always)]` is what makes the CALL SITES fold rather than merely their bodies. That is
// the difference between "the panel path is unchanged" as prose and as a measurement: with the
// real `hold` compiled in, LLVM kept an out-of-line call at every bracket and the knob-off image
// MOVED (x86 +1496 B, arm +896 B, `./arroyo knoboff beam` exit 1, orin 27's first gate run). The
// call sites in `wm.rs`/`strip.rs` stay ungated and unmoved — no column shifts, no `#[cfg]` noise
// in the present path — because the folding happens here.

/// Off-knob [`hold`]: no beam source exists, so no bracket does. Constant `None`.
#[cfg(not(feature = "beam"))]
#[inline(always)]
pub fn hold(_y0: usize, _y1: usize, _panel_h: usize, _slow: bool, _record: bool) -> Option<Hold> {
    None
}

/// Off-knob [`settle`]: only a `None` bracket can reach here, and it closes to nothing.
#[cfg(not(feature = "beam"))]
#[inline(always)]
pub fn settle(_h: Option<Hold>) -> Option<Obs> {
    None
}

/// Off-knob [`take_last`]: no bracket ever parked an observation, so every witness reads `blind`
/// and falls back to the duration predicate it used before this module existed.
#[cfg(not(feature = "beam"))]
#[inline(always)]
pub fn take_last() -> Option<Obs> {
    None
}
