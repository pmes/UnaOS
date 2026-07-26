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

//! WC-G — the window-path garble, localized to ONE of three suspects, on the wire.
//!
//! ## What the verdict chain already settled (do not re-derive here)
//!
//! WC-D read a window's content back through `info.stride` from the scan-out and found it
//! byte-correct. WC-E ordered the writers. WC-F, on silicon, found every scan-out identity true
//! (base / row step / pitch / panel / fits), both twin blocks clean (`comp_bad=0 direct_bad=0`,
//! 8192/8192 checked), and photographed the ramp straight — the HVS steps exactly `k_row`.
//! Everything GLOBAL is therefore exonerated. Yet a live 128x128 window still garbles, with
//! horizontal, band-shaped smearing that gets *better* when the app cycles faster.
//!
//! Every one of those instruments shares a property: it measures **converged** content. A one-shot
//! read-back, a static twin, a photographed ramp — each is sampled after the writing stopped. A
//! window that repaints forever never converges, and that is precisely the population that garbles.
//! WC-G instruments the non-converged case: the present path *while it is running*.
//!
//! ## The three suspects, and the leg that separates each
//!
//! Four checksums of the SAME surface, taken at four moments around one blit, plus a read-back of
//! what that blit actually landed:
//!
//! | leg | when | what a divergence means |
//! |-----|------|-------------------------|
//! | `app` | at `SYS_WIN_PRESENT` entry, before the composite | the app's declared frame |
//! | `blit` | immediately before `draw_window` reads it | `app != blit` ⇒ the surface moved between the app's present and the copy |
//! | `civac` | after `DC CIVAC` over the surface | `blit != civac` ⇒ **coherency**: the kernel's lines did not match the coherent view |
//! | `after` | immediately after `draw_window` returns | `blit != after` ⇒ **race**: the owner wrote the surface *mid-copy* (tearing at the source) |
//! | `fbbad` | source re-derived and compared against `read_pixel` over the content rect | non-zero ⇒ the **blit/upscale** path put the wrong pixel somewhere |
//!
//! That table is the whole instrument. `blit != civac` and `blit != after` cannot both be explained
//! by one mechanism, which is what makes suspect 2 and suspect 1 separable rather than a shrug.
//!
//! **What `coher` can and cannot claim.** The compositor's own read of the surface is a normal
//! cacheable read of Normal Inner-Shareable memory, and the caches are PIPT: against another core's
//! cacheable writes to the same PA it is coherent *by construction*, and no witness is needed to say
//! so. What the `civac` leg actually tests is narrower and is the part that is not guaranteed — an
//! **alias-attribute mismatch**: the surface reached through two mappings whose memory attributes or
//! shareability disagree (the EL0 `user_data_page` leaf vs the kernel's identity leaf), or a
//! non-cacheable/mismatched alias somewhere in the chain, either of which puts the two views outside
//! the architecture's coherency guarantee. So `coher=0` means "no alias-attribute mismatch on this
//! surface", NOT "coherency is fine in general" — the latter was never in doubt for the plain
//! same-attribute case, and reading the counter as a broader clearance would overclaim.
//!
//! **Why `CIVAC` here, when WC-D was required to use a bare `IVAC`.** WC-D's rule was about the
//! FRAMEBUFFER, which the kernel itself writes: cleaning it before reading would have written the
//! blit's own dirty lines out and healed the very short-flush defect the witness existed to catch —
//! an instrument that lies. The surface is the mirror image. The kernel only ever READS it; there
//! are no kernel-dirty lines to write back, so `CIVAC` cannot repair a compositor defect. What it
//! can do is force the next read to come from the coherent view, which is exactly the question.
//! A bare `IVAC` would additionally risk DISCARDING the owner's un-cleaned lines — destroying app
//! data to answer a question `CIVAC` answers safely. The rulings differ because the buffers differ.
//!
//! ## `own=` — the leg the bench observation demanded
//!
//! Two EL0 apps with unrelated paint loops (the uvug crystal's 300-frame renderer and `stat.elf`'s
//! trivial ~20 fps counter repaint) garble the same way, so the defect is in the SHARED path. This
//! witness runs per WINDOW ID, not for window 0, so that claim is provable on the wire rather than
//! asserted.
//!
//! It also records *why* this window was being blitted. `own=yes`: the blit follows this window's
//! own `SYS_WIN_PRESENT`, so its owner is parked inside the syscall and cannot be writing.
//! `own=no`: this window was repainted as **collateral** — the damage set is closed upward over
//! occlusion, so presenting window A repaints every higher-z window that overlaps it. In that case
//! B's owner is running free at EL0 with nothing at all serialising it against the copy of its
//! surface. `own=no` with `blit != after` is the source-tearing mechanism caught in the act, and it
//! is reachable only because two windows overlap — which is the configuration the bench runs.
//!
//! ## `us=` / `slow=` — the fourth answer, the one the checksums cannot give
//!
//! If all four checksums agree and `fbbad=0`, every byte was correct at every moment and the panel
//! still garbles. Then the defect is not in WHAT was written but in WHEN. `draw_window` writes
//! per-pixel, with `put_pixel`, **directly into the front framebuffer** — the live scan-out. It is
//! not double-buffered: the desktop reaches the panel through `Screen`'s back buffer and a
//! contiguous per-row damage-rect flush, but a window's pixels are poked one at a time into the
//! memory the HVS is scanning *right now*, with no vblank synchronisation anywhere in the path.
//!
//! `us=` is how long that copy takes. `rectscan_us=` is how long the beam spends on the window's own
//! destination rows (`frame_us * rows / panel_height`) — the threshold that matters, because the
//! scan-out only has to cross THIS RECT to latch it part-old and part-new. `slow=yes` therefore
//! means the overtake is *guaranteed*, not merely likely, and it happens at whatever scanline the
//! beam held when the copy passed it: a horizontal band boundary. That is the shape in the
//! photograph. It also explains "cycling faster looks a little better" — a faster cycle does not
//! remove the tear, it shortens the interval any one torn frame stays on the panel.
//!
//! WC-G does not fix anything. It says which of the four it is, with a number for each.
//!
//! **Read `slow=` with WC-H in mind.** The paragraph above describes the path as WC-G found it, and
//! that description is now historical: WC-H gave the window layer a back buffer, so `draw_window`
//! composes off-screen and reaches the panel through contiguous row copies. WC-G's bracket still
//! contains the copy and nothing else, but the copy is now two phases and only the second is visible
//! to the scan-out — so `slow=yes` means "the whole operation outran the beam", not "the panel
//! tore". The tear question lives in [`stage_note`]'s `[wc-h] torn=`, which measures the present
//! phase alone. The four checksum legs are unaffected and keep their original meanings.
//!
//! **WC-K lives here too.** The back-layer discipline WC-H built for a window's pixels was extended
//! by WC-K to the desktop FILL that a close, a move or a re-tile paints over a vacated box — the last
//! writer in the window lifecycle that still poked the live front buffer per pixel. Its witness
//! ([`erase_note`] / [`erase_decline`]) is in this module because it answers the same question with
//! the same units and the same thresholds, and splitting it into a fourth file would only let the two
//! `rectscan_us` derivations drift apart.
//!
//! ## Scope and cost
//!
//! `witness`-gated AND aarch64-only, like `wcf`: knob-off this module does not compile and the
//! flashable Pi media are byte-identical. Budgeted at [`SAMPLES`] instrumented blits per window id
//! and silent thereafter — the checksums are 64 KiB reads and the read-back is one probe per source
//! pixel, from present context at EL0 frame rates, so an unbudgeted version would perturb the very
//! timing it reports. Every sample prints; the terminal `verdict` line is one-shot.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use super::FrameBuffer;
use crate::arch::aarch64::{cache, now_cycles};

/// Instrumented blits per window id. Four is enough to distinguish a steady state from a one-off
/// and small enough that the checksum reads do not dominate the interval being timed.
const SAMPLES: u32 = 4;

/// Window ids this witness tracks. Matches `wm::MAX_WINDOWS`; ids at or above it are not sampled.
const IDS: usize = 8;

/// One 60 Hz frame, in microseconds. The bench panel is 60 Hz; a slower panel only makes the
/// derived `rectscan_us` larger and the reported `slow=yes` more conservative.
const FRAME_US: u64 = 16_667;

/// FNV-1a 64 offset basis. Also the value a null or empty surface hashes to, which no drawn surface
/// produces — so an all-basis line reads as "nothing was mapped", not as "the frames agreed".
const FNV_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Per-id: the checksum recorded at that window's last `SYS_WIN_PRESENT` entry.
static APP_CKS: [AtomicU64; IDS] = [const { AtomicU64::new(FNV_BASIS) }; IDS];
/// Per-id: how many presents that window has entered. Compared against [`SEEN_SEQ`] to decide
/// whether a blit is the window's own present or a collateral repaint.
static APP_SEQ: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
/// Per-id: the present sequence the last instrumented blit of that window observed.
static SEEN_SEQ: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
/// Per-id: instrumented blits taken so far, capped at [`SAMPLES`].
static TAKEN: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];

/// Per-window tallies, rolled up into that window's own `rollup` line.
///
/// **There is no global "all windows are done" summary, and that is a deliberate reversal.** Two
/// cuts of one were tried and both were dishonest in the same way. The first fired when the FIRST
/// window spent its budget — printing the summary before window 2 had been sampled at all, including
/// before its `own=no` collateral-repaint sample, so a metal race on that path would have sat on the
/// wire UNDERNEATH a green `CLEAN+SLOW` verdict the spec REQUIREs. The second tried "every window
/// that has been SAMPLED has spent its budget", which is the same bug in different clothes: the
/// sampled set only holds windows seen so far, so the predicate is trivially true the instant the
/// first window finishes. It reproduced exactly, printing `scope=exhausted samples=4 windows=1`
/// before window 2 existed. A quiescence timer was tried third and also fired early — the gate's two
/// apps start more than 3 s apart, so an idle gap is not evidence that sampling is over.
///
/// The lesson is structural, not a tuning problem: **nothing observable inside the boot can
/// distinguish "sampling is finished" from "the next app has not launched yet."** Any global summary
/// is therefore a completeness claim the instrument cannot support, and a summary that overstates
/// its scope is worse than no summary — it is the one artifact that can make later contrary evidence
/// look already accounted for.
///
/// So the rollup is scoped to ONE window and fires when that window spends its budget: deterministic,
/// no timer, and its scope is exactly what its `win=` says. The job the global line was reaching for
/// — "no suspect fired anywhere, ever" — belongs to the spec instead, as FORBIDs on the suspect
/// verdicts. A FORBID needs no completeness claim: it catches an anomaly in any window at any point
/// in the boot, including one that appears long after every rollup has printed.
static W_SAMPLES: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
static W_COHER: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
static W_RACE: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
static W_BLIT: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
static W_CLEAN: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
static W_SLOW: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
static W_MAXUS: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];

// ---- WC-H — the back-layer's own witness -------------------------------------------------------

/// Per-id: `[wc-h]` samples taken, capped at [`SAMPLES`].
static H_TAKEN: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
/// Per-id: samples whose PRESENT phase alone outran the beam's time on the box.
static H_TORN: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
/// Per-id: the largest present-phase duration seen, in microseconds.
static H_MAXPRES: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];
/// Per-id: composites that did NOT reach the back layer and ran on the direct (pre-WC-H) path — the
/// tearing regime. Excludes the deliberate fixture decline, which is counted separately.
static H_DECLINE: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
/// Per-id: declines the KERNEL asked for, to keep the fallback path covered ([`DECL_FIXTURE`]).
static H_FIXTURE: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
/// Per-id: a recorded-but-not-yet-printed sample. See [`stage_flush`] for why the print is deferred.
static H_PEND: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
/// Per-id: what the pending sample IS — [`KIND_STAGED`] or one of the decline reasons.
static H_KIND: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];

/// The pending sample is a staged composite; the timing fields are meaningful.
const KIND_STAGED: u32 = 0;
/// Decline reasons, in the order [`super::wm::stage_window`] can produce them.
pub const DECL_GEOM: u32 = 1;
pub const DECL_CAP: u32 = 2;
pub const DECL_LOCK: u32 = 3;
pub const DECL_ALLOC: u32 = 4;
/// Not a failure: the witness build's deliberate one-shot fallback, so WC-D verifies the direct path
/// at least once per boot. Counted apart from the real declines and never affects the verdict.
pub const DECL_FIXTURE: u32 = 5;

fn decl_name(kind: u32) -> &'static str {
    match kind {
        DECL_GEOM => "geom",
        DECL_CAP => "cap",
        DECL_LOCK => "lock",
        DECL_ALLOC => "alloc",
        DECL_FIXTURE => "fixture",
        _ => "?",
    }
}

/// WC-H — record a composite that did NOT reach the back layer.
///
/// **Why this exists, and what it fixes.** The first cut of this witness fired only on staged
/// success, and the verdict it printed was an overclaim in a way that mattered: `stage_window` has
/// four fall-back exits — a box over [`super::wm::MAX_STAGE_BYTES`], a `try_lock` lost to another
/// core, an allocator that will not grow the buffer, degenerate geometry — and each one silently
/// runs the DIRECT, pre-WC-H path, which is the tearing regime this arc exists to leave. A boot in
/// which 96 of 100 composites lost the lock to a concurrent desktop flush (a ~6 ms hold window, which
/// the compositor's own note names as expected contention) would have torn continuously and still
/// printed `TEAR-FREE` from its four staged samples, with the FORBID never firing. The same blind
/// spot would hide a window whose box exceeds the cap falling back *permanently*.
///
/// So a decline is a SAMPLE, not a non-event: it spends budget, it prints its own line with its
/// reason, and it makes the rollup say `UNSTAGED` instead of `TEAR-FREE`. The cap fallback becomes
/// loud for free.
pub fn stage_decline(id: u32, reason: u32) {
    let i = id as usize;
    if i >= IDS {
        return;
    }
    let n = H_TAKEN[i].fetch_add(1, Ordering::Relaxed) + 1;
    if n > SAMPLES {
        H_TAKEN[i].store(SAMPLES + 1, Ordering::Relaxed);
        // Past budget the LINE stops but the count must not: an unstaged composite is the thing the
        // verdict is about, and a boot that starts declining after sample 4 has to remain visible.
        if reason == DECL_FIXTURE {
            H_FIXTURE[i].fetch_add(1, Ordering::Relaxed);
        } else {
            H_DECLINE[i].fetch_add(1, Ordering::Relaxed);
        }
        return;
    }
    if reason == DECL_FIXTURE {
        H_FIXTURE[i].fetch_add(1, Ordering::Relaxed);
    } else {
        H_DECLINE[i].fetch_add(1, Ordering::Relaxed);
    }
    H_KIND[i].store(reason, Ordering::Relaxed);
    H_PEND[i].store(n, Ordering::Release);
}
static H_BOX: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];
static H_BYTES: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];
static H_COMPOSE: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];
static H_PRESENT: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];
static H_RECTSCAN: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];

/// WC-H — report one staged window composite: how long the off-screen compose took, how long the
/// row-copy present into the live scan-out took, and whether that present alone is still longer than
/// the beam's time on the window's box.
///
/// ### Why this line exists next to `[wc-g] slow=`
///
/// WC-G's `us=` brackets the whole of `draw_window`, and WC-H did not change what that bracket
/// means: `blit` is still the surface as the copy found it, `after` still as the copy left it, and
/// the interval still contains the copy and nothing else. What WC-H changed is what the copy DOES —
/// it is now a compose into cached RAM followed by a present, and only the second half is visible to
/// the scan-out. So `[wc-g] slow=yes` after WC-H no longer means "the panel will tear": it means
/// "the whole operation outran the beam", most of which the beam cannot observe. That is why the
/// `slow` leg is not re-scoped and the WC-G FORBIDs are untouched — the checksum verdicts they guard
/// are unaffected — and why the tear question moves HERE, to the only phase that can still tear.
///
/// `torn=yes` is therefore the honest, narrowed successor to `slow=yes`: the present phase measured
/// against `rectscan_us`, computed exactly as WC-G computes it (`FRAME_US * rows / panel_height`,
/// with the same deliberate bias toward NOT reporting a tear — the frame period includes blanking
/// the beam does not spend on visible rows, so the real scan time of the box is shorter than the
/// figure it is compared against, and a `torn=no` near the threshold is not a proof of safety).
///
/// Budgeted per window id like the rest of this module, and the rollup is scoped to ONE window for
/// the same reason: nothing observable inside a boot can tell "sampling finished" from "the next app
/// has not launched yet", so a global summary would be a completeness claim the instrument cannot
/// support. See [`W_SAMPLES`].
#[allow(clippy::too_many_arguments)]
pub fn stage_note(
    id: u32,
    bw: usize,
    bh: usize,
    bytes: usize,
    t_end: u64,
    t0: u64,
    t1: u64,
    panel_h: usize,
) {
    let i = id as usize;
    if i >= IDS {
        return;
    }
    let n = H_TAKEN[i].fetch_add(1, Ordering::Relaxed) + 1;
    if n > SAMPLES {
        H_TAKEN[i].store(SAMPLES + 1, Ordering::Relaxed);
        return;
    }
    let compose_us = cycles_to_us(t1.saturating_sub(t0));
    let present_us = cycles_to_us(t_end.saturating_sub(t1));
    let rectscan_us = if panel_h == 0 { 0 } else { FRAME_US * bh as u64 / panel_h as u64 };
    if present_us > rectscan_us {
        H_TORN[i].fetch_add(1, Ordering::Relaxed);
    }
    H_MAXPRES[i].fetch_max(present_us, Ordering::Relaxed);
    H_KIND[i].store(KIND_STAGED, Ordering::Relaxed);
    H_BOX[i].store(((bw as u64) << 32) | bh as u64, Ordering::Relaxed);
    H_BYTES[i].store(bytes as u64, Ordering::Relaxed);
    H_COMPOSE[i].store(compose_us, Ordering::Relaxed);
    H_PRESENT[i].store(present_us, Ordering::Relaxed);
    H_RECTSCAN[i].store(rectscan_us, Ordering::Relaxed);
    H_PEND[i].store(n, Ordering::Release);
}

/// WC-H — print whatever [`stage_note`] recorded for `id`, if anything.
///
/// **Why the print is deferred to here instead of happening at the measurement.** The first cut
/// emitted the `[wc-h]` line from inside `stage_window`, which is inside `draw_window`, which is
/// inside WC-G's clock — so every serial character of this witness was charged to `[wc-g] us=`. It
/// showed up immediately and unmistakably: `us=` rose from a baseline max of 15524 to 23468 on the
/// same work, while the compose and present numbers the line itself reported summed to about half of
/// that. An instrument that inflates the measurement it appears next to is worse than no
/// instrument — the arc's own cost delta would have been unreadable.
///
/// So the sample is RECORDED where it is taken and PRINTED from here, which the compositor calls
/// after `wcg::end` has stopped the clock. Both witnesses then measure only the copy.
///
/// **The printed lines are a SUBSET of the samples, by design.** There is one pending slot per id, so
/// if two cores composite the same window concurrently (a present on one, a desktop-flush repaint on
/// the other) the second recording overwrites the first before either is printed, and one line is
/// lost. That is why the per-sample line count can be less than the rollup's `samples=`: the counters
/// the rollup reports — `torn`, `maxpresent_us` — are updated at RECORD time and miss nothing, while
/// the per-sample lines are a best-effort trace. A queue would fix the trace at the cost of putting
/// allocation or a lock on the present path, which is not a trade this witness is worth.
pub fn stage_flush(id: u32) {
    let i = id as usize;
    if i >= IDS {
        return;
    }
    let n = H_PEND[i].swap(0, Ordering::AcqRel);
    if n == 0 {
        return;
    }
    let kind = H_KIND[i].load(Ordering::Relaxed);
    if kind == KIND_STAGED {
        let bx = H_BOX[i].load(Ordering::Relaxed);
        let present_us = H_PRESENT[i].load(Ordering::Relaxed);
        let rectscan_us = H_RECTSCAN[i].load(Ordering::Relaxed);
        serial_println!(
            "[wc-h] win={} box={}x{} bytes={} compose_us={} present_us={} rectscan_us={} torn={} -> BUFFERED",
            id,
            bx >> 32,
            bx & 0xFFFF_FFFF,
            H_BYTES[i].load(Ordering::Relaxed),
            H_COMPOSE[i].load(Ordering::Relaxed),
            present_us,
            rectscan_us,
            if present_us > rectscan_us { "yes" } else { "no" }
        );
    } else {
        // A composite that ran on the pre-WC-H direct path. `-> DIRECT` deliberately does NOT carry
        // the rollup's verdict strings: one decline is a fact to report, not a boot to fail, and the
        // FORBIDs sit on the rollup where the aggregate lives.
        serial_println!("[wc-h] win={} staged=no reason={} -> DIRECT", id, decl_name(kind));
    }
    if n == SAMPLES {
        let torn_n = H_TORN[i].load(Ordering::Relaxed);
        let decl_n = H_DECLINE[i].load(Ordering::Relaxed);
        // Precedence: a measured tear outranks an unmeasured one. `UNSTAGED` is not a lesser verdict
        // — it says composites reached the panel through the unbuffered path, so the TEAR-FREE claim
        // the staged samples support does not cover the window's actual behaviour. `fixture` is
        // excluded from `declines` because the kernel asked for it (see `DECL_FIXTURE`); it is
        // printed separately so the exclusion is visible rather than assumed.
        let verdict = if torn_n > 0 {
            "AT-RISK"
        } else if decl_n > 0 {
            "UNSTAGED"
        } else {
            "TEAR-FREE"
        };
        serial_println!(
            "[wc-h] rollup win={} scope=window samples={} torn={} declines={} fixture={} maxpresent_us={} frame_us={} -> {}",
            id,
            n,
            torn_n,
            decl_n,
            H_FIXTURE[i].load(Ordering::Relaxed),
            H_MAXPRES[i].load(Ordering::Relaxed),
            FRAME_US,
            verdict
        );
    }
}

// ---- WC-K — the staged DESKTOP FILL's own witness ----------------------------------------------

/// Staged erase fills to report per boot. The erase path has no window id to budget against — an
/// erase belongs to a box that no longer has an owner — so the budget is global, and four is the
/// same figure the per-window witnesses use for the same reason: enough to tell a steady state from
/// a one-off, few enough that the serial writes do not perturb a close path.
const E_SAMPLES: u32 = 4;

/// Decline lines to print before going quiet. Deliberately LARGER than [`E_SAMPLES`], and
/// deliberately NOT the same budget.
///
/// WC-H's `stage_decline` shares one budget between successes and declines, which leaves a real
/// blind spot: a boot that stages its first four composites and declines every one thereafter stops
/// printing, and the rollup that already fired cannot retract. That is survivable there because a
/// window composites continuously and the counters keep moving. It is not survivable here, because
/// the FORBID on a direct fill is this arc's whole verdict — WC-G convicted this exact writing
/// pattern, and a direct fill that appears only after the rollup has printed must still fail the
/// gate. So a decline is printed whether or not the sample budget is spent, and only a boot that
/// declines pathologically (more than this many) goes quiet, with the counter still running.
const E_DECL_LINES: u32 = 16;

/// Staged fills recorded so far, capped at [`E_SAMPLES`] for line purposes.
static E_TAKEN: AtomicU32 = AtomicU32::new(0);
/// Staged fills whose PRESENT phase alone outran the beam's time on the box.
static E_TORN: AtomicU32 = AtomicU32::new(0);
/// Staged fills whose present was not `h` contiguous, row-stepped runs. Structural, not timing.
static E_NONCONTIG: AtomicU32 = AtomicU32::new(0);
/// Fills that fell back to the direct `fill_rect` — the pre-WC-K tearing regime.
static E_DECLINE: AtomicU32 = AtomicU32::new(0);
/// Decline lines emitted, capped at [`E_DECL_LINES`].
static E_DECL_LINES_OUT: AtomicU32 = AtomicU32::new(0);
/// The largest present-phase duration seen across staged fills, in microseconds.
static E_MAXPRES: AtomicU64 = AtomicU64::new(0);
/// Total panel rows presented by staged fills — the size of what this discipline now covers.
static E_ROWS: AtomicU64 = AtomicU64::new(0);

/// WC-K — record a desktop fill that did NOT reach the back layer and ran the direct `fill_rect`.
///
/// The reasons are [`super::wm::stage_window`]'s, reused rather than duplicated: the two staging
/// paths decline for the same four causes and a separate vocabulary would only invite the two to
/// drift.
pub fn erase_decline(w: usize, h: usize, reason: u32) {
    E_DECLINE.fetch_add(1, Ordering::Relaxed);
    let n = E_DECL_LINES_OUT.fetch_add(1, Ordering::Relaxed) + 1;
    if n > E_DECL_LINES {
        E_DECL_LINES_OUT.store(E_DECL_LINES + 1, Ordering::Relaxed);
        return;
    }
    serial_println!(
        "[wc-k] erase box={}x{} staged=no reason={} -> DIRECT",
        w,
        h,
        decl_name(reason)
    );
}

/// WC-K — report one staged desktop fill: the box, the composed row, how long the compose and the
/// row-copy present took, whether the present alone outran the beam's time on those rows, and
/// whether the present was in fact `h` contiguous row-stepped runs.
///
/// ### Why `contig=` is a leg and not a comment
///
/// The tear-free claim WC-H established rests on a specific SHAPE — bulk `copy_nonoverlapping` runs,
/// one per scanline, each wholly inside its row — and not on the mere fact that a staging buffer was
/// involved. A staged path whose present dribbled out in fragments, or whose runs overhung into the
/// next scanline, would report perfectly good `compose_us`/`present_us` numbers and still be back in
/// the regime this arc removes. So the shape is CHECKED at the present (see
/// [`super::wm::stage_fill`]) and reported here, and the spec FORBIDs `contig=no` independently of
/// the timing verdict.
///
/// `rectscan_us` is computed exactly as [`stage_note`] and WC-G compute it, with the same deliberate
/// bias toward NOT reporting a tear: `FRAME_US` includes blanking the beam does not spend on visible
/// rows, so the box's real scan time is shorter than the figure the present is compared against and
/// a `torn=no` near the threshold is not a proof of safety.
#[allow(clippy::too_many_arguments)]
pub fn erase_note(
    w: usize,
    h: usize,
    row_bytes: usize,
    contig: bool,
    t_end: u64,
    t0: u64,
    t1: u64,
    panel_h: usize,
) {
    let compose_us = cycles_to_us(t1.saturating_sub(t0));
    let present_us = cycles_to_us(t_end.saturating_sub(t1));
    let rectscan_us = if panel_h == 0 { 0 } else { FRAME_US * h as u64 / panel_h as u64 };
    let torn = present_us > rectscan_us;
    if torn {
        E_TORN.fetch_add(1, Ordering::Relaxed);
    }
    if !contig {
        E_NONCONTIG.fetch_add(1, Ordering::Relaxed);
    }
    E_MAXPRES.fetch_max(present_us, Ordering::Relaxed);
    E_ROWS.fetch_add(h as u64, Ordering::Relaxed);
    let n = E_TAKEN.fetch_add(1, Ordering::Relaxed) + 1;
    if n > E_SAMPLES {
        // Past budget the LINE stops and the counters above keep running, so a tear or a fragmented
        // present that only appears late in the boot is still counted — and, because the declines
        // print unbudgeted, still reachable by a FORBID.
        E_TAKEN.store(E_SAMPLES + 1, Ordering::Relaxed);
        return;
    }
    serial_println!(
        "[wc-k] erase box={}x{} staged=yes rowbytes={} runs={} contig={} compose_us={} present_us={} rectscan_us={} torn={} -> BUFFERED",
        w,
        h,
        row_bytes,
        h,
        if contig { "yes" } else { "no" },
        compose_us,
        present_us,
        rectscan_us,
        if torn { "yes" } else { "no" }
    );
    if n == E_SAMPLES {
        // Precedence, most specific first: a present that was not the right SHAPE invalidates the
        // tear-free argument regardless of what the clock said, so `SPLIT` outranks a measured tear;
        // a measured tear outranks an unmeasured one; `UNSTAGED` means fills reached the panel
        // through the direct path and the staged samples do not cover the erase path's behaviour.
        //
        // `scope=fills` is not `scope=boot`, and the distinction is WC-G's lesson repeated: nothing
        // observable inside a boot can distinguish "the erase path is finished" from "the next app
        // has not closed a window yet", so this line claims only the fills it has SEEN. The
        // "did a direct fill ever happen, anywhere, at any point" question is the spec's FORBID on
        // `-> DIRECT`, which needs no completeness claim and which the unbudgeted decline lines keep
        // reachable for the whole boot.
        let torn_n = E_TORN.load(Ordering::Relaxed);
        let nc = E_NONCONTIG.load(Ordering::Relaxed);
        let decl = E_DECLINE.load(Ordering::Relaxed);
        let verdict = if nc > 0 {
            "SPLIT"
        } else if torn_n > 0 {
            "AT-RISK"
        } else if decl > 0 {
            "UNSTAGED"
        } else {
            "TEAR-FREE"
        };
        serial_println!(
            "[wc-k] rollup scope=fills samples={} rows={} torn={} noncontig={} declines={} maxpresent_us={} frame_us={} -> {}",
            n,
            E_ROWS.load(Ordering::Relaxed),
            torn_n,
            nc,
            decl,
            E_MAXPRES.load(Ordering::Relaxed),
            FRAME_US,
            verdict
        );
    }
}

/// FNV-1a 64 over a mapped surface slot, bounded by the length the MAPPING code supplied — the same
/// bound `draw_window` reads under, so the checksum can never walk past the slot. Volatile, so the
/// four calls around one blit cannot be folded into one by the compiler; that folding would turn
/// every divergence this witness exists to find into a silent agreement.
fn checksum(surf: usize, surf_len: usize) -> u64 {
    let mut h = FNV_BASIS;
    if surf == 0 {
        return h;
    }
    let p = surf as *const u8;
    let mut i = 0usize;
    while i < surf_len {
        // SAFETY: `i < surf_len`, the real byte length of the mapped slot.
        h ^= unsafe { core::ptr::read_volatile(p.add(i)) } as u64;
        h = h.wrapping_mul(FNV_PRIME);
        i += 1;
    }
    h
}

/// CNTVCT_EL0 ticks converted to microseconds at the generic timer's own rate. `CNTFRQ_EL0` is read
/// per call (a system register read, cheap next to a 64 KiB checksum) and falls back to the Pi 4's
/// 54 MHz if the firmware left it zero, so a bad `CNTFRQ` degrades the number rather than dividing
/// by zero.
fn cycles_to_us(dt: u64) -> u64 {
    let frq: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) frq, options(nomem, nostack, preserves_flags));
    }
    let frq = if frq == 0 { 54_000_000 } else { frq };
    dt.saturating_mul(1_000_000) / frq
}

/// Whether any window still has budget. Gates the present-side checksum so the instrument stops
/// costing anything once it has said what it has to say.
fn budget_left() -> bool {
    (0..IDS).any(|i| TAKEN[i].load(Ordering::Relaxed) < SAMPLES)
}

/// Record the app-side frame at `SYS_WIN_PRESENT` entry — the checksum of what the owner declared
/// finished, taken while the owner is parked inside the syscall and provably not writing. Called
/// from `wm::present`, after the table lock is dropped and before the composite.
pub fn on_present(id: u32, surf: usize, surf_len: usize) {
    let i = id as usize;
    if i >= IDS || !budget_left() {
        return;
    }
    APP_CKS[i].store(checksum(surf, surf_len), Ordering::Relaxed);
    APP_SEQ[i].fetch_add(1, Ordering::Relaxed);
}

/// State carried across one instrumented blit. Not `Copy`: it exists exactly between [`begin`] and
/// [`end`], and there is nothing sensible to do with a duplicate.
pub struct Probe {
    id: u32,
    seq: u32,
    own: bool,
    surf: usize,
    surf_len: usize,
    cks_app: u64,
    cks_blit: u64,
    cks_civac: u64,
    t0: u64,
}

/// Open a sample: take the `blit` checksum (the surface exactly as `draw_window` is about to read
/// it), then the `civac` checksum (the same bytes through the coherent view), and start the clock.
///
/// Returns `None` when this window is out of budget, is a compat row (chrome-less legacy shim — not
/// the path under investigation), or has no mapped surface. Call [`end`] with the returned probe
/// immediately after `draw_window` returns, and with nothing in between: the `after` checksum's
/// meaning is "the surface as it stood when the copy finished", and any work inserted between the
/// two widens the window it measures.
pub fn begin(id: u32, surf: usize, surf_len: usize, compat: bool) -> Option<Probe> {
    let i = id as usize;
    if compat || surf == 0 || surf_len == 0 || i >= IDS {
        return None;
    }
    if TAKEN[i].fetch_add(1, Ordering::Relaxed) >= SAMPLES {
        // Saturate rather than wrap: the counter is also the budget test in `budget_left`.
        TAKEN[i].store(SAMPLES, Ordering::Relaxed);
        return None;
    }
    let cks_blit = checksum(surf, surf_len);
    // The coherency leg. See the module note on why this cleans as well as invalidates.
    cache::clean_invalidate_range(surf, surf_len);
    let cks_civac = checksum(surf, surf_len);

    let seq = APP_SEQ[i].load(Ordering::Relaxed);
    let own = SEEN_SEQ[i].swap(seq, Ordering::Relaxed) != seq;

    Some(Probe {
        id,
        seq,
        own,
        surf,
        surf_len,
        cks_app: APP_CKS[i].load(Ordering::Relaxed),
        cks_blit,
        cks_civac,
        // LOAD-BEARING ORDERING: the clock starts AFTER the `civac` re-read, and must stay there.
        // `clean_invalidate_range` drops every line of the surface, and the `cks_civac` checksum
        // immediately re-reads all of it — which is precisely what re-warms the source for the blit
        // that follows. Start `t0` before that re-read (say, alongside `cks_blit`) and `us=` silently
        // absorbs a full 64 KiB cache refill that the uninstrumented path never pays, inflating the
        // very number the timing verdict rests on. The measured interval must contain the copy and
        // nothing else.
        t0: now_cycles(),
    })
}

/// Close a sample: stop the clock, take the `after` checksum, re-derive the destination from the
/// source and compare it against the scan-out, and print the line.
///
/// The geometry arguments are `draw_window`'s own, passed rather than re-read so the read-back
/// walks EXACTLY the pixels the blit wrote — a witness that recomputed the bounds could disagree
/// with the blit about which pixels exist and report that disagreement as a defect.
#[allow(clippy::too_many_arguments)]
pub fn end(p: Probe, fb: &FrameBuffer, x: usize, y: usize, w: usize, h: usize, stride: usize, scale: usize) {
    let us = cycles_to_us(now_cycles().saturating_sub(p.t0));
    let cks_after = checksum(p.surf, p.surf_len);

    // Re-derive the destination from the source, one probe per SOURCE pixel (the top-left
    // destination pixel of each upscale cell). Bounds mirror `draw_window`'s: the panel clip, the
    // stride column bound, and the `surf_len` row bound.
    let info = fb.info();
    let (pw, ph) = (info.width, info.height);
    let mut checked = 0usize;
    let mut bad = 0usize;
    if x < pw && y < ph && scale > 0 && stride >= 4 {
        let cols = (pw - x).div_ceil(scale).min(w).min(stride / 4);
        let rows = (ph - y).div_ceil(scale).min(h).min(p.surf_len / stride);
        for row in 0..rows {
            let row_base = row * stride;
            for col in 0..cols {
                // SAFETY: identical bound to `draw_window`'s read —
                // `row < surf_len / stride` and `col < stride / 4`.
                let want = unsafe {
                    core::ptr::read_unaligned((p.surf as *const u8).add(row_base + col * 4) as *const u32)
                } & 0x00FF_FFFF;
                if let Some(got) = fb.read_pixel(x + col * scale, y + row * scale) {
                    checked += 1;
                    if got != want {
                        bad += 1;
                    }
                }
            }
        }
    }

    // Attribution, most specific first. A source that moved under the copy invalidates the
    // read-back's expectation (it was re-derived from bytes the blit never saw), so RACE outranks
    // BLIT rather than being reported alongside it — `fbbad` is still printed, so the raw number is
    // never hidden by the verdict drawn from it.
    let w = p.id as usize;
    let verdict = if p.cks_blit != p.cks_civac {
        W_COHER[w].fetch_add(1, Ordering::Relaxed);
        "COHER"
    } else if p.cks_blit != cks_after {
        W_RACE[w].fetch_add(1, Ordering::Relaxed);
        "RACE-BLIT"
    } else if p.own && p.cks_app != p.cks_blit {
        W_RACE[w].fetch_add(1, Ordering::Relaxed);
        "RACE-PRESENT"
    } else if bad != 0 {
        W_BLIT[w].fetch_add(1, Ordering::Relaxed);
        "BLIT"
    } else {
        W_CLEAN[w].fetch_add(1, Ordering::Relaxed);
        "CLEAN"
    };
    // The tearing criterion. NOT "longer than a frame" — that threshold is arbitrary and sits on a
    // knife edge. The beam only has to cross THIS WINDOW'S rows for the copy to be latched
    // part-old/part-new, so the honest threshold is the time the HVS spends scanning the window's
    // destination rows: `frame_us * rows_on_panel / panel_height`. A blit longer than that is
    // guaranteed — not merely likely — to have the scan-out overtake it somewhere inside the rect,
    // and the artifact that produces is a horizontal band boundary at whatever scanline the beam
    // held when the copy passed it. There is no vblank synchronisation anywhere in this path to
    // prevent it: `draw_window` pokes the live front buffer the moment a present asks it to.
    //
    // The estimate errs toward NOT reporting a tear. `FRAME_US` is the whole frame period, blanking
    // included, but `ph` counts only VISIBLE lines — so this divides the blanking interval among the
    // visible rows and credits the rect with beam time the beam does not actually spend on it. The
    // real scan time of the rect is therefore SHORTER than `rectscan_us`, every `slow=yes` is
    // conservative, and a `slow=no` near the threshold may still be tearing.
    let rows_dst = (h * scale).min(ph.saturating_sub(y));
    let rectscan_us = if ph == 0 { 0 } else { FRAME_US * rows_dst as u64 / ph as u64 };
    let slow = us > rectscan_us;
    if slow {
        W_SLOW[w].fetch_add(1, Ordering::Relaxed);
    }
    let n = W_SAMPLES[w].fetch_add(1, Ordering::Relaxed) + 1;
    W_MAXUS[w].fetch_max(us, Ordering::Relaxed);

    serial_println!(
        "[wc-g] win={} seq={} own={} scale={}x app={:#018x} blit={:#018x} civac={:#018x} after={:#018x} fbbad={}/{} us={} rectscan_us={} slow={} -> {}",
        p.id,
        p.seq,
        if p.own { "yes" } else { "no" },
        scale,
        p.cks_app,
        p.cks_blit,
        p.cks_civac,
        cks_after,
        bad,
        checked,
        us,
        rectscan_us,
        if slow { "yes" } else { "no" },
        verdict
    );

    // This window's rollup, once, when it spends its budget. Scoped to ONE window and deterministic
    // — no timer, and no claim about any window but this one. See [`W_SAMPLES`] for why there is no
    // global summary and why the "did any suspect ever fire" question is the spec's FORBIDs instead.
    if n == SAMPLES {
        let coher = W_COHER[w].load(Ordering::Relaxed);
        let race = W_RACE[w].load(Ordering::Relaxed);
        let blitn = W_BLIT[w].load(Ordering::Relaxed);
        let clean = W_CLEAN[w].load(Ordering::Relaxed);
        let slown = W_SLOW[w].load(Ordering::Relaxed);
        // Same precedence as the per-sample verdict. `CLEAN+SLOW` is the load-bearing outcome: every
        // byte correct at every moment, and the unbuffered per-pixel copy into the live scan-out
        // still longer than the beam's time on the rect — the timing suspect, which no checksum can
        // reach and which any of the other three verdicts would have masked.
        let dominant = if coher > 0 {
            "COHER"
        } else if race > 0 {
            "RACE"
        } else if blitn > 0 {
            "BLIT"
        } else if slown > 0 {
            "CLEAN+SLOW"
        } else {
            "CLEAN"
        };
        serial_println!(
            "[wc-g] rollup win={} scope=window samples={} coher={} race={} blit={} clean={} slow={} maxus={} frame_us={} -> {}",
            p.id,
            n,
            coher,
            race,
            blitn,
            clean,
            slown,
            W_MAXUS[w].load(Ordering::Relaxed),
            FRAME_US,
            dominant
        );
    }
}
