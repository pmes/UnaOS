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
//! shareability disagree (the user-mode `user_data_page` leaf vs the kernel's identity leaf), or a
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
//! Two user apps with unrelated paint loops (the uvug crystal's 300-frame renderer and `stat.elf`'s
//! trivial ~20 fps counter repaint) garble the same way, so the defect is in the SHARED path. This
//! witness runs per WINDOW ID, not for window 0, so that claim is provable on the wire rather than
//! asserted.
//!
//! It also records *why* this window was being blitted. `own=yes`: the blit follows this window's
//! own `SYS_WIN_PRESENT`, so its owner is parked inside the syscall and cannot be writing.
//! `own=no`: this window was repainted as **collateral** — the damage set is closed upward over
//! occlusion, so presenting window A repaints every higher-z window that overlaps it. In that case
//! B's owner is running free in user mode with nothing at all serialising it against the copy of its
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
//! ([`erase_note`] / [`erase_defer`] / [`erase_drop`]) is in this module because it answers the same
//! question with the same units and the same thresholds, and splitting it into a fourth file would
//! only let the two `rectscan_us` derivations drift apart. WC-L then removed that path's direct
//! fallback outright, so the fill either stages or is owed — never written to the live front buffer.
//!
//! ## Scope and cost
//!
//! `witness`-gated, and gated on nothing else: knob-off this module does not compile and every
//! flashable artifact — Pi media and x86 ESP alike — is byte-identical with it absent. It was
//! aarch64-only until the two primitives it needed acquired arch-neutral spellings
//! ([`crate::arch::now_cycles`], already both-arch, and [`clean_invalidate_surface`] below); the
//! WC-L erase discipline the `[wc-k]` lines report is arch-neutral and always was, so reporting it
//! on one arch only meant the x86 compositor ran that discipline with no witness over it. The
//! module therefore COMPILES on both arches, and the other half followed: every `wcg::` call site in
//! [`super::wm`] now carries a plain `#[cfg(feature = "witness")]` too, so an x86 witness build
//! emits these lines and a banded present is distinguishable from a whole-box one on both arches.
//! Two neighbouring sites deliberately stayed aarch64-only, because neither is a `wcg::` call: the
//! WC-F reserved-box interlock talks to the VideoCore mailbox, and WC-L's deferral fixture drives a
//! Pi-shaped stage path that needs a native x86 fixture instead. Budgeted at [`SAMPLES`]
//! instrumented blits per window id
//! and silent thereafter — the checksums are 64 KiB reads and the read-back is one probe per source
//! pixel, from present context at user-mode frame rates, so an unbudgeted version would perturb the very
//! timing it reports. Every sample prints; the terminal `verdict` line is one-shot.
//!
//! FBCON-DMG splits that budget in two for `[wc-h]` alone: [`SAMPLES`] WHOLE-BOX samples and
//! [`SAMPLES`] BANDED ones per window id, because a single budget is spent by whichever class arrives
//! first and the first class is always whole-box (window creation, first paint). See [`H_BTAKEN`].
//! The tallies behind them are unbudgeted and the rollup is one line per budget, so the wire can say
//! `banded=0` as readily as it can say `banded=980`.
//!
//! ## WC-H2 — the rollup had to outlive its own budget
//!
//! Three defects in that arrangement were found by reading two consecutive metal boots
//! (`rmbp-gr15-s70`), and all three are the same mistake wearing different clothes: **a line that
//! fires when a BUDGET spends can only ever describe the moment the budget spent.**
//!
//! 1. *It fired seconds into the boot and never again.* The banded rollup latches at
//!    `H_BTAKEN >= SAMPLES` — four banded presents — which in boot A of that capture landed at line
//!    519 of 1904, moments after console routing began. [`H_MINSPAN`], [`H_BANDED`] and [`H_WHOLE`]
//!    are UNBUDGETED and keep counting for the whole boot, but nothing ever printed them again. The
//!    consequence is on the wire: boot A's line read `banded=1 minspan=736` of a 750-row box, and a
//!    session read it and concluded banding buys the console nothing. Boot B's `scope=window` line
//!    happened to fire late (line 2796 of 3119 — luck, not design: that window's whole-box budget
//!    filled after its banded one) and read `banded=271 minspan=96`. Ninety-six rows of seven hundred
//!    and fifty: 13% of the surface, and the exact opposite conclusion. The instrument reported a
//!    startup burst and was believed.
//! 2. *`samples=` printed the CONSTANT.* The print site passed [`SAMPLES`] rather than any counter,
//!    so `samples=4` was uninformative on every rollup this module has ever emitted — it restated the
//!    budget's definition. It now comes from the live budget counter, and [`H_LINES`] carries the
//!    count that genuinely differs from it (see below).
//! 3. *Budgeted and unbudgeted quantities shared one line with no marking.* `torn=7` appeared beside
//!    `samples=4` and read as "7 of 4". Worse than the confusion was the truth underneath it: `torn`
//!    was NOT a whole-boot tally either. The tear test sat INSIDE the budget gate, so it could count
//!    at most `2 * SAMPLES` — 8 — over an entire boot, and a window that composited cleanly for its
//!    first eight presents and tore for the next thousand printed `torn=0 -> TEAR-FREE`. The
//!    `AT-RISK` verdict, and the spec FORBID that rests on it, were budget-limited.
//!
//! The fix is three-part and every part is load-bearing on its own:
//!
//! - **The tear test and `maxpresent_us` move OUT of the budget gate** ([`stage_note`]), so `torn=`
//!   becomes a genuine whole-boot census and `AT-RISK` is reachable at any point in the boot. Cost is
//!   two subtractions, a multiply and a divide per present — arithmetic, no serial, no checksum, so
//!   the "unbudgeted version would perturb the timing" objection that governs the CHECKSUMS does not
//!   reach it.
//! - **The rollup RE-FIRES** ([`census_refresh`]), so those whole-boot censuses are actually read
//!   after the console has run. Neither half is sufficient alone: an unbudgeted `torn` that is only
//!   ever printed at second four is still invisible, and a re-fired line carrying a budget-capped
//!   `torn` still cannot say more than 8.
//! - **Every field says which population it is drawn from**, via repeated `pop=` markers. See
//!   [`stage_rollup`].
//!
//! **What the re-fired line does NOT claim.** [`W_SAMPLES`]'s note is the standing law here and it is
//! not repealed: nothing observable inside a boot can distinguish "this window is finished
//! compositing" from "the next line has not been printed yet", so no line in this module may claim
//! completeness. A refreshed rollup claims strictly less than that — it is a RUNNING census with the
//! moment it was taken stamped on it (`age_ms=`) and its ordinal among this window's rollups
//! (`emit=`). The reader's rule is the one that follows from that: for any `win=`, the line with the
//! greatest `emit=` supersedes every earlier one, and rollup lines are never summed. They are
//! snapshots of monotone totals, not deltas, which is also why re-emission cannot double-count
//! anything — see [`census_refresh`].

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use super::FrameBuffer;
use crate::arch::now_cycles;

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

/// WC-G/M1 — per-id: microseconds this window's SAMPLES spent inside the witness itself, summed
/// across every sampled pass. `cks_blit_us + civac_us + cks_after_us + readback_us`, the four phases
/// the `prof` line decomposes.
///
/// **What it is for, and why it is a per-window total rather than a per-sample field.** The `us=`
/// leg times the copy and deliberately excludes everything this module does around it, which is
/// correct for the tearing verdict and useless for the question M1 asks: an armed x86 boot spends
/// ~2.87 s per instrumented pass and ~11.5 s of a 17.3 s block on four of them, against ~1.4 s of
/// real GPU work. That cost is invisible on every existing line precisely BECAUSE the brackets are
/// honest. `wit_us=` is the one number that says how much of the boot the instrument bought itself,
/// and it belongs on the rollup because the interesting quantity is the window's total, not any one
/// pass's share of it.
///
/// It is a LOWER bound on the witness's true cost and must be read as one. The serial writes — the
/// sample line, and now the `prof` line — sit outside every bracket that feeds this sum, by the same
/// rule that keeps `stage_flush`'s print outside WC-G's clock. So `wit_us=` is the measurement cost,
/// not the reporting cost, and the reporting cost is stated separately at [`end`].
static W_WITUS: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];

// ---- WC-H — the back-layer's own witness -------------------------------------------------------

/// Per-id: `[wc-h]` samples taken, capped at [`SAMPLES`].
static H_TAKEN: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
/// Per-id: presents whose PRESENT phase alone outran the beam's time on the box.
///
/// **Unbudgeted since WC-H2, and that is the whole of its meaning.** The tear test used to sit below
/// the two budget gates in [`stage_note`], so this counter could not exceed `2 * SAMPLES` no matter
/// what the boot did — eight, forever. `AT-RISK` and the pi4 spec's FORBID on it were therefore
/// scoped to a window's first handful of presents while reading as a claim about the window. It is
/// now taken on every present, before either gate.
static H_TORN: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
/// Per-id: the largest present-phase duration seen, in microseconds. Unbudgeted since WC-H2, for the
/// same reason as [`H_TORN`]: a maximum over four startup presents is not a maximum over the boot,
/// and it was being printed as one.
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
/// Per-id: whether the pending sample's present was BANDED — 1 banded, 0 whole-box. Carried beside
/// [`H_KIND`] rather than folded into it because a decline has no span at all to classify.
static H_BAND: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
/// Per-id: the pending sample's span — the rows the present actually wrote, which is `bh` for a
/// whole-box present and strictly less for a banded one.
static H_SPAN: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];

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
    mark_seen(i);
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

// ---- FBCON-DMG — telling a banded present from a whole-box one ----------------------------------

/// Per-id: BANDED `[wc-h]` sample lines taken, capped at [`SAMPLES`]. A budget of its OWN, and that
/// separation is the entire reason this counter exists.
///
/// FBCON-DMG made a console present write only the rows its damage covers, and the first boot that
/// carried it could not prove so on the wire. The reason is arithmetic rather than subtlety: the four
/// samples [`H_TAKEN`] budgets are spent by whatever composites arrive FIRST, and what arrives first
/// is window creation and first paint — whole-box presents, every one of them. The ~980 damage-banded
/// console presents that followed were past budget, so [`stage_note`] returned before recording
/// anything, and the feature's whole observable footprint was four lines describing the one case it
/// does not change. **An instrument whose budget is spent by the control can never see the
/// treatment**, and that made the feature unfalsifiable rather than merely unreported.
///
/// So a banded present spends a different budget. A window may burn all four whole-box samples during
/// creation and still record four banded samples afterwards, which is exactly the population
/// FBCON-DMG exists to alter. Declines keep sharing [`H_TAKEN`] with the whole-box samples: a decline
/// never reached the banding loop, so it has no span to classify and no claim to the banded budget.
static H_BTAKEN: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
/// Per-id: banded presents RECORDED, and unbudgeted — the count is taken before either budget test,
/// so it keeps running long after the lines stop. The lines are a trace; this is the census, and a
/// boot that bands 980 times must not report 4 just because 4 is all it printed.
static H_BANDED: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
/// Per-id: whole-box presents recorded, unbudgeted, for the same reason and to the same standard.
/// Kept as its own counter rather than derived from `samples - banded` because the two budgets make
/// that subtraction wrong — declines spend [`H_TAKEN`] too, and they are neither.
static H_WHOLE: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
/// Per-id: the NARROWEST banded present seen, packed `span << 32 | bytes`. `u64::MAX` means no banded
/// present has been recorded at all, which the rollup prints as `minspan=0 minspan_bytes=0` beside
/// `banded=0` rather than inventing a width.
///
/// Packed into ONE atomic rather than kept as two, so `fetch_min` yields a span together with the
/// byte count that BELONGS to it. Two independent minima would be free to pair the span of one
/// present with the bytes of another and report a cost no present ever had — the sort of composite
/// figure that reads as a measurement and is not one. `span` occupies the high half so the ordering
/// is by span; `bytes` is `row_bytes * span` at a fixed window width, so the low half can only ever
/// agree with that ordering, never invert it.
static H_MINSPAN: [AtomicU64; IDS] = [const { AtomicU64::new(u64::MAX) }; IDS];

// ---- WC-H2 — the state a REFRESHED rollup needs, and nothing more ------------------------------

/// Per-id: `now_cycles()` at this window's first recorded present or decline, or 0 before there is
/// one. The origin `age_ms=` is measured from, and the reason it is per-WINDOW rather than global:
/// the gate's apps start more than 3 s apart, so a boot-relative age would say more about launch
/// order than about how long this window has been compositing.
///
/// `age_ms=` is the field that separates defect 1's two readings on sight. Boot A's `banded=1
/// minspan=736` was taken at an age of milliseconds; boot B's `banded=271 minspan=96` at an age of
/// seconds. Nothing on the old line distinguished them, and the serial log carries no timestamps of
/// its own, so the distinction had to be manufactured here or not at all.
static H_T0: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];

/// Per-id: `now_cycles()` as of the END of this window's most recent rollup emission — after the
/// serial write, deliberately, so [`CENSUS_PERIOD_US`] bounds the *duty cycle* the refresh imposes on
/// the composite path rather than merely the interval between its starts.
///
/// Also the refresh's mutual exclusion: it is armed by a `compare_exchange` on this cell, so of two
/// cores flushing the same window at the same instant exactly one prints. See [`census_refresh`].
static H_LASTROLL: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];

/// Per-id: [`census_total`] as of the most recent rollup emission. The refresh's other gate: a window
/// whose censuses have not moved has nothing new to say, and reprinting an unchanged line would spend
/// serial time to restate the previous one. An idle window therefore goes quiet with its last line
/// describing its last active state, which is the correct steady-state report for it.
static H_LASTCENSUS: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];

/// Per-id: rollup lines emitted for this window so far. Printed as `emit=`, one-based.
///
/// Not decoration. `emit=1` standing as the ONLY rollup for a window whose censuses are still
/// growing is how this arc's own machinery is falsified from the wire: it says the refresh never
/// armed — [`stage_flush`] unreached, the clock reading zero, or the delta gate stuck — and it says
/// so without needing a second boot to compare against.
static H_EMIT: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];

/// Per-id: per-sample `[wc-h]` lines actually printed by [`emit_sample`]. Printed as `lines=`.
///
/// This is the count that genuinely diverges from `samples=`. There is one pending slot per id, so a
/// concurrent composite can overwrite a recorded sample before it is printed, and the module has
/// always documented that the trace is a subset — but it left the reader to notice the shortfall by
/// counting lines by hand. The `rmbp-gr15-s70` capture has it: window 3's first rollup follows three
/// `-> BUFFERED` lines and announces four samples. `lines=3 samples=4` states the loss instead of
/// leaving it to arithmetic, and `lines=0` alongside a non-zero `samples=` would say the trace path
/// is dead rather than merely lossy.
///
/// **It is a WINDOW count, and `samples=` is a SCOPE count — so `lines` may legitimately EXCEED it.**
/// A window emits sample lines against both budgets and for its declines, so the ceiling here is
/// `2 * SAMPLES`, while `samples=` never exceeds one budget. The same capture shows the shape: window
/// 1's `scope=window` rollup in the newer boot follows SEVEN per-sample lines, four whole-box and
/// three of the four banded records (one overwritten). `lines=7 samples=4` is not a contradiction and
/// must not be read as one; the comparison `lines=` is for is against `2 * budget=`, and the
/// comparison `samples=` is for is against the `pop=all-presents` census beside it.
static H_LINES: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];

/// Minimum wall time between two rollup emissions for one window.
///
/// The refresh's whole cost is serial. A rollup line is ~250 bytes and the UART runs at 115200 baud,
/// so one line is ~20 ms of a core's time on the composite path — the same path whose microsecond
/// timings this module exists to report, which is why the figure cannot simply be made small. Two
/// seconds puts one window's refresh at a ~1% duty cycle and the worst case ([`IDS`] windows all
/// compositing hard) at ~8%.
///
/// It is also what bounds the residual error this arc does NOT remove. The last refreshed line of a
/// boot is taken up to `CENSUS_PERIOD_US` of presents before the final one, so its censuses are a
/// LOWER BOUND on the boot's true totals, short by at most one period's worth. Against the ~27
/// presents/second the `rmbp-gr15-s70` console sustained that is a tail of up to ~54 of ~271 — where
/// the defect being fixed reported 1 of 271. The bound is stated rather than hidden because there is
/// no honest way to close it from inside this file: a genuinely final census needs a boot-end or
/// shell-verb call site, and every such site lives in another module.
const CENSUS_PERIOD_US: u64 = 2_000_000;

/// Per-id rollup latches. Bit [`ROLL_WHOLE`] for the whole-box budget's rollup, bit [`ROLL_BAND`] for
/// the banded one; each fires at most once per window per boot.
///
/// A latch, and not the old `if n == SAMPLES` test against the pending slot. There is one pending slot
/// per id ([`stage_flush`]), so a concurrent composite can overwrite the very sample that would have
/// carried `n == SAMPLES` — and where losing a trace line is the documented, acceptable cost of not
/// putting a queue on the present path, losing a ROLLUP is not: it is the line the spec's FORBIDs sit
/// on. The latch reads the budget counter instead, which no overwrite can disturb, so the rollup
/// fires on whichever flush first observes the budget spent.
static H_ROLLED: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
const ROLL_WHOLE: u32 = 1 << 0;
const ROLL_BAND: u32 = 1 << 1;

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
///
/// ### FBCON-DMG — `bh` and `span` are two arguments, not one
///
/// `span` is the rows this present WROTE; `bh` is the height of the box they live in. The present is
/// **banded** exactly when `span < bh` and **whole-box** when `span == bh`, and until this signature
/// carried both, nothing downstream could tell the two apart — `bh` was the parameter's name and
/// `span` was the value the sole call site passed into it, which made every banded present look like
/// a short window. That is why this line could confirm FBCON-DMG's cost but never its absence, and
/// why the fix had to reach the classification rather than only the print.
///
/// `rectscan_us` stays derived from `span`, not `bh`, and deliberately so: the tearing threshold is
/// the beam's time on the rows the present actually touches, and crediting a banded present with the
/// whole box's scan time would raise the bar it is measured against and hide tears. That is unchanged
/// behaviour — `span` is the value this argument slot always held.
#[allow(clippy::too_many_arguments)]
pub fn stage_note(
    id: u32,
    bw: usize,
    bh: usize,
    span: usize,
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
    mark_seen(i);
    // Classify and census FIRST, before either budget test. Past budget the LINE stops but the count
    // must not — the same rule `stage_decline` already follows, and here it is the difference between
    // "this window banded 4 times" and the truth.
    let banded = span < bh;
    if banded {
        H_BANDED[i].fetch_add(1, Ordering::Relaxed);
        H_MINSPAN[i].fetch_min(((span as u64) << 32) | (bytes as u64 & 0xFFFF_FFFF), Ordering::Relaxed);
    } else {
        H_WHOLE[i].fetch_add(1, Ordering::Relaxed);
    }
    // WC-H2 — the TEAR TEST is a census too, and it belongs on this side of the budget gate for the
    // same reason `banded` does.
    //
    // It used to sit below, inside the gate, which capped `H_TORN` at `2 * SAMPLES` — eight — for a
    // whole boot. A window that composited cleanly through its first eight presents and then tore for
    // the next thousand printed `torn=0` and the rollup called it `TEAR-FREE`, with the spec's FORBID
    // on `-> AT-RISK` never able to fire. The verdict was scoped to the startup burst while reading
    // like a claim about the window.
    //
    // The module's standing objection to unbudgeted work does not reach here. What it protects is the
    // CHECKSUMS — 64 KiB volatile reads and a per-pixel read-back, which would perturb the very
    // interval they are timing. This is two saturating subtractions, one multiply and one divide on
    // values the caller already handed us; it touches no memory outside two atomics and writes
    // nothing to the UART. `maxpresent_us` moves for the same reason and at the same price: a maximum
    // over a window's first four presents is precisely the startup-burst reading this arc exists to
    // stop reporting as a steady state.
    let present_us = cycles_to_us(t_end.saturating_sub(t1));
    let rectscan_us = if panel_h == 0 { 0 } else { FRAME_US * span as u64 / panel_h as u64 };
    if present_us > rectscan_us {
        H_TORN[i].fetch_add(1, Ordering::Relaxed);
    }
    H_MAXPRES[i].fetch_max(present_us, Ordering::Relaxed);
    // Two budgets, one per class. See [`H_BTAKEN`] for why a shared one made the feature invisible.
    let n = if banded {
        let n = H_BTAKEN[i].fetch_add(1, Ordering::Relaxed) + 1;
        if n > SAMPLES {
            H_BTAKEN[i].store(SAMPLES + 1, Ordering::Relaxed);
            return;
        }
        n
    } else {
        let n = H_TAKEN[i].fetch_add(1, Ordering::Relaxed) + 1;
        if n > SAMPLES {
            H_TAKEN[i].store(SAMPLES + 1, Ordering::Relaxed);
            return;
        }
        n
    };
    // Only the per-SAMPLE line's fields are computed past the gate now; `present_us` and
    // `rectscan_us` were taken above, where the census needs them.
    let compose_us = cycles_to_us(t1.saturating_sub(t0));
    H_KIND[i].store(KIND_STAGED, Ordering::Relaxed);
    H_BAND[i].store(banded as u32, Ordering::Relaxed);
    H_BOX[i].store(((bw as u64) << 32) | bh as u64, Ordering::Relaxed);
    H_SPAN[i].store(span as u64, Ordering::Relaxed);
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
///
/// WC-H2 puts that shortfall on the wire instead of leaving it to be counted by hand: the rollup's
/// `lines=` is how many per-sample lines this window actually emitted, against its `samples=`. See
/// [`H_LINES`].
///
/// **The ROLLUPS are not part of that best-effort trace, and no longer ride on the pending slot.**
/// They are latched off the budget counters instead ([`H_ROLLED`]), so an overwritten sample can cost
/// a trace line but never the line the spec's verdicts are read from. There are two of them, one per
/// budget — see [`stage_rollup`] for what each one's `scope=` claims and what it deliberately does
/// not.
pub fn stage_flush(id: u32) {
    let i = id as usize;
    if i >= IDS {
        return;
    }
    let n = H_PEND[i].swap(0, Ordering::AcqRel);
    if n != 0 {
        emit_sample(id, i);
    }
    // FBCON-DMG — the whole-box rollup fires as it always did, at the first flush that observes the
    // whole-box/decline budget spent. The banded one is new and fires on its own budget, which in a
    // real boot is LATER: window creation spends the whole-box budget before the console has printed a
    // line. A boot that never bands therefore never emits a `scope=window-band` line at all, and a
    // boot that bands emits exactly one. That PRESENCE-OR-ABSENCE is the falsification test, and it is
    // what WC-H2 left untouched when it stopped relying on these one-shots for the census.
    //
    // What it did NOT leave untouched is the belief that one line each was enough. Both latches fire
    // when a four-sample BUDGET spends, which is seconds into the boot, and the censuses they carried
    // — `banded`, `minspan`, `whole`, and now `torn` — go on counting for the rest of it. So the
    // `scope=window` line is kept current by `census_refresh` below, and the reader takes the greatest
    // `emit=` for each `win=`.
    if H_TAKEN[i].load(Ordering::Relaxed) >= SAMPLES
        && H_ROLLED[i].fetch_or(ROLL_WHOLE, Ordering::Relaxed) & ROLL_WHOLE == 0
    {
        stage_rollup(id, i, "window", H_TAKEN[i].load(Ordering::Relaxed));
    }
    if H_BTAKEN[i].load(Ordering::Relaxed) >= SAMPLES
        && H_ROLLED[i].fetch_or(ROLL_BAND, Ordering::Relaxed) & ROLL_BAND == 0
    {
        stage_rollup(id, i, "window-band", H_BTAKEN[i].load(Ordering::Relaxed));
    }
    // WC-H2 — and then, for the rest of the boot, keep the censuses on those lines readable.
    census_refresh(id, i);
}

/// WC-H2 — note that this window has been seen, once, so `age_ms=` has an origin.
///
/// `compare_exchange` from 0 rather than a plain store: the origin must be the FIRST record, and a
/// later store would silently reset the age and make every subsequent rollup look like a startup
/// burst — reintroducing the defect from the other end. `now_cycles()` returning 0 exactly once at
/// boot would leave the origin unset and the age reading as the raw uptime, which is conservative in
/// the right direction (it can only make a line look OLDER, never younger, so it cannot manufacture
/// the "this is steady state" reading).
#[inline]
fn mark_seen(i: usize) {
    if H_T0[i].load(Ordering::Relaxed) == 0 {
        let _ = H_T0[i].compare_exchange(0, now_cycles(), Ordering::Relaxed, Ordering::Relaxed);
    }
}

/// WC-H2 — every present, decline and fixture this window has had, budget or no budget. The quantity
/// the refresh's delta gate is taken over, and the denominator a reader should hold `samples=` up
/// against.
#[inline]
fn census_total(i: usize) -> u32 {
    H_WHOLE[i]
        .load(Ordering::Relaxed)
        .wrapping_add(H_BANDED[i].load(Ordering::Relaxed))
        .wrapping_add(H_DECLINE[i].load(Ordering::Relaxed))
        .wrapping_add(H_FIXTURE[i].load(Ordering::Relaxed))
}

/// WC-H2 — re-emit this window's `scope=window` rollup so its UNBUDGETED censuses are read after the
/// console has actually run.
///
/// ### Why this had to exist
///
/// See the module note's WC-H2 section for the two boots that convict the old arrangement. The short
/// form: `H_MINSPAN`, `H_BANDED`, `H_WHOLE`, `H_DECLINE` and (now) `H_TORN` count for the whole boot,
/// and every one of them was printed exactly once, at the arbitrary instant a four-sample budget
/// spent. One boot printed `banded=1 minspan=736` and another printed `banded=271 minspan=96` for the
/// SAME window doing the SAME work, because the two budgets happened to fill in a different order.
///
/// ### Why `scope=window`, and only `scope=window`
///
/// Every census field on the line is per-WINDOW; only `samples=`/`budget=` belong to the scope. The
/// two scopes therefore print identical censuses, and refreshing both would double the serial cost to
/// restate the same numbers. `scope=window` is the window's line — the one the pi4 spec REQUIREs and
/// the one the FORBIDs sit on — so that is the one kept current. `scope=window-band` keeps its
/// original job unchanged: a one-shot marker for the moment the banded budget spent, whose presence
/// or absence is still the "did this window ever band" falsification test the [`stage_rollup`] note
/// describes.
///
/// ### Reachability, and how it is guaranteed rather than assumed
///
/// [`stage_flush`] is the sole call site and `video::wm`'s composite pass calls it once per window
/// per pass, immediately after `wcg::end` — the same place that already prints every `[wc-h]` sample
/// line. So the refresh runs on exactly the population it reports on: a window that is compositing is
/// a window whose flush is being called. It needs no new call site, no timer and no thread, and there
/// is nothing to keep alive between passes.
///
/// The corollary matters as much: a window that STOPS compositing stops refreshing. The last line for
/// such a window describes its last active state, not a later moment, and `age_ms=` says which.
///
/// ### Why re-emission cannot double-count
///
/// Every quantity on the line is a MONOTONE TOTAL, incremented exactly once per event at record time,
/// before any budget or emission test. Nothing printed here is a delta and nothing is reset by
/// printing it. A rollup line is therefore a snapshot, and re-taking a snapshot of a counter cannot
/// add to it — the arithmetic that would double-count belongs to a reader who SUMS lines, which is
/// why the module note states the reader's rule (`greatest emit= per win=` wins; never sum) and why
/// `emit=` is on the line at all.
///
/// Two cores flushing the same window at the same instant would be the one way to get two lines
/// bearing the same census, which is not double-counting but is noise. The `compare_exchange` on
/// [`H_LASTROLL`] excludes it: the loser observed the same `last` and its swap fails, so it returns
/// without printing.
fn census_refresh(id: u32, i: usize) {
    // Never BEFORE the window's own first rollup. The refresh is a continuation of that line, not a
    // second instrument, and firing early would put a `scope=window` line on the wire before the
    // budget it reports had been spent — a line whose `samples=` was honestly short but which the
    // spec's REQUIRE would match all the same.
    if H_ROLLED[i].load(Ordering::Relaxed) & ROLL_WHOLE == 0 {
        return;
    }
    // Delta gate: nothing new to say, say nothing. This is what keeps an idle desktop silent.
    let total = census_total(i);
    if total == H_LASTCENSUS[i].load(Ordering::Relaxed) {
        return;
    }
    // Rate gate. Checked before the `compare_exchange` so the common case — a window compositing at
    // frame rate, arriving here dozens of times a second — costs one clock read and a compare.
    let last = H_LASTROLL[i].load(Ordering::Relaxed);
    let now = now_cycles();
    if cycles_to_us(now.saturating_sub(last)) < CENSUS_PERIOD_US {
        return;
    }
    // Arm: exactly one core proceeds. See the note above.
    if H_LASTROLL[i].compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed).is_err() {
        return;
    }
    stage_rollup(id, i, "window", H_TAKEN[i].load(Ordering::Relaxed));
}

/// Print the one pending `[wc-h]` sample line for window `id`.
///
/// **The key order here is load-bearing across seats.** `win=`, `compose_us=`, `present_us=`, `torn=`
/// and the terminal `-> BUFFERED` are matched, in that order, by another platform track's regression
/// gate. Fields may be INSERTED between them — FBCON-DMG's `span=` and `band=` are — but none of those
/// five may be renamed, reordered, or moved off the end without a paired spec change on that side.
fn emit_sample(id: u32, i: usize) {
    // WC-H2 — count the line, not the sample. `samples=` counts what was RECORDED; this counts what
    // reached the wire, and the gap between them is the documented pending-slot overwrite. See
    // [`H_LINES`]. Incremented before the print so a line lost to a panic mid-`serial_println!` is
    // reported as attempted rather than as never taken.
    H_LINES[i].fetch_add(1, Ordering::Relaxed);
    let kind = H_KIND[i].load(Ordering::Relaxed);
    if kind == KIND_STAGED {
        let bx = H_BOX[i].load(Ordering::Relaxed);
        let present_us = H_PRESENT[i].load(Ordering::Relaxed);
        let rectscan_us = H_RECTSCAN[i].load(Ordering::Relaxed);
        // FBCON-DMG — `box=` is the WHOLE box again, and `span=` the rows this present wrote. Before
        // the split those were one number and it was the span, so a banded present of a 66x780 box
        // reported `box=66x78` and was indistinguishable on the wire from a whole-box present of a
        // short window. `bytes=` is unchanged and still `row_bytes * span` — what reached the glass.
        // On aarch64 no band is ever produced, so `span == bh`, `box=` prints exactly what it always
        // printed, and `band=no` on every line.
        serial_println!(
            "[wc-h] win={} box={}x{} span={} band={} bytes={} compose_us={} present_us={} rectscan_us={} torn={} -> BUFFERED",
            id,
            bx >> 32,
            bx & 0xFFFF_FFFF,
            H_SPAN[i].load(Ordering::Relaxed),
            if H_BAND[i].load(Ordering::Relaxed) != 0 { "yes" } else { "no" },
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
}

/// One window's `[wc-h] rollup` line, fired once per budget by [`stage_flush`].
///
/// `scope=window` is the whole-box-and-decline budget's rollup and is the line that existed before
/// FBCON-DMG, with its verdict, its precedence and its leading fields untouched. `scope=window-band`
/// is the banded budget's, printed by the same code with the same counters so the two can never drift
/// into disagreeing about a shared quantity.
///
/// ### What `banded=` claims, and what it does not
///
/// `whole=` and `banded=` are the UNBUDGETED censuses — every present [`stage_note`] classified, not
/// the handful it printed — so a console that bands a thousand times says a thousand. What neither
/// number claims is completeness: nothing observable inside a boot can distinguish "this window is
/// finished compositing" from "the next line has not been printed yet" (see [`W_SAMPLES`]), so the
/// honest reading is always "as of `age_ms=`", never "for the boot".
///
/// WC-H2 changed WHEN they are read, not what they claim. They used to be printed once, at the
/// arbitrary instant a four-sample budget spent — which is why boot A of `rmbp-gr15-s70` reported
/// `banded=1 minspan=736` for the same window that boot B reported `banded=271 minspan=96` for. The
/// line is now refreshed while the window composites ([`census_refresh`]), so `banded=0` on the
/// GREATEST-`emit=` `scope=window` line means "this window had not banded as of `age_ms=`", and
/// paired with the ABSENCE of a `scope=window-band` line anywhere in the boot it is the wire's way of
/// saying the window never banded at all. That pair remains the falsification test: no band line for
/// the negative, exactly one for the positive.
///
/// `minspan=` is the narrowest band seen and `minspan_bytes=` the bytes that band actually copied,
/// taken together from one packed atomic ([`H_MINSPAN`]) so they always describe the same present.
/// Both read `0` when nothing banded, which `banded=0` on the same line disambiguates from a real
/// zero-row present — a present of no rows does not exist, `stage_window` declines on `bh == 0`.
///
/// Banding does NOT enter the verdict precedence, and that is deliberate. A banded present is the
/// feature working, not a defect; folding it into `AT-RISK`/`UNSTAGED`/`TEAR-FREE` would change what
/// the spec's existing FORBIDs mean on a line they already guard.
///
/// ### WC-H2 — `pop=`, and why the marking is positional
///
/// The old line put `samples=4` and `torn=7` side by side with nothing to say they were counted over
/// different populations, and `torn=7` on a line whose budget was 4 read as "7 of 4". Every field now
/// sits under a `pop=` marker naming the population it is drawn from, and the marker governs
/// everything up to the next one:
///
/// | marker | covers | means |
/// |--------|--------|-------|
/// | `pop=budgeted` | `samples` `budget` | THIS SCOPE's budget only — at most [`SAMPLES`] events |
/// | `pop=all-presents` | `torn` … `maxpresent_us` | every present/decline this WINDOW has had |
/// | `pop=constant` | `frame_us` | a compile-time constant, not a measurement |
///
/// `pop=` repeats rather than each field carrying its own suffix because the alternative was to
/// RENAME `torn=`, `whole=`, `banded=` and the rest, and another platform track's regression gate
/// reads this line: `win=`, `scope=`, `declines=` and the terminal `-> TEAR-FREE` are matched by
/// `unaos/scripts/specs/pi4-regression.spec`, which permits fields to be INSERTED between them and
/// nothing else. Markers are insertions. A naive key/value parser that folds a line into a map will
/// keep only the last `pop=` and lose the others — accepted knowingly, because such a parser loses
/// only the markers and no field's value, and the markers are meant to be read positionally.
///
/// ### `samples=` is a counter now, and what it can and cannot tell you
///
/// It used to print [`SAMPLES`] — the constant — so it restated the budget's own definition and
/// `samples=4` carried no information on any rollup ever emitted. It now comes from the live budget
/// counter for the scope that fired, clamped to `budget=`.
///
/// Be precise about what that fixes. The rollup fires only when its budget is spent, so on a line
/// that exists `samples` still necessarily EQUALS `budget` — the honest gain is that it is now a
/// reading rather than an assertion, and that the pairing makes the real comparison visible: 4
/// sampled out of the `whole + banded + declines` the census reports. The field that genuinely
/// diverges is `lines=`, which counts what reached the wire; see [`H_LINES`].
///
/// ### What a FAIL reads like
///
/// The point of the refresh is that these all stay reachable for the whole boot instead of the first
/// four presents:
///
/// - `-> AT-RISK` — `torn > 0`. Now countable past the budget and now printed after the console has
///   run, so a window that tears only under load says so. The pi4 spec FORBIDs it.
/// - `-> UNSTAGED` — `declines > 0`; a composite reached the panel through the pre-WC-H direct path.
///   Also FORBIDden.
/// - `banded=0` on the latest line of a window the console is known to be routing damage into —
///   FBCON-DMG is not reaching the compositor at all.
/// - `minspan=` at or near the box height with a large `banded=` — banding is happening and buying
///   nothing. This is the reading a session drew from boot A's `banded=1 minspan=736`; the difference
///   is that `emit=` and `age_ms=` now say whether the line describes a startup burst or a steady
///   state, so the same conclusion would this time be supportable or refutable rather than guessed.
/// - `emit=1` as a window's only rollup while its censuses are still growing — the refresh itself is
///   broken, and the line is back to describing the first four presents.
/// - `lines=0` beside a non-zero `samples=` — the per-sample trace path is dead, not merely lossy.
fn stage_rollup(id: u32, i: usize, scope: &str, taken: u32) {
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
    let ms = H_MINSPAN[i].load(Ordering::Relaxed);
    let (minspan, minbytes) = if ms == u64::MAX { (0, 0) } else { (ms >> 32, ms & 0xFFFF_FFFF) };
    // `age_ms=` from this window's own origin. A window with no origin yet cannot reach here (both
    // recorders call `mark_seen` before touching a budget), so the 0 branch is the unreachable-clock
    // case and reads as an age of zero rather than as the raw uptime.
    let t0 = H_T0[i].load(Ordering::Relaxed);
    let age_ms = if t0 == 0 { 0 } else { cycles_to_us(now_cycles().saturating_sub(t0)) / 1000 };
    let emit = H_EMIT[i].fetch_add(1, Ordering::Relaxed) + 1;
    // KEY ORDER IS LOAD-BEARING ACROSS SEATS. `win=`, `scope=`, `declines=` and the terminal
    // `-> {verdict}` are matched in this order by the pi4 track's regression spec, which also relies
    // on `scope=window ` carrying a TRAILING SPACE so its pattern cannot match `scope=window-band`.
    // Everything WC-H2 added — `emit=`, `age_ms=`, the `pop=` markers, `budget=`, `lines=` — is an
    // INSERTION between existing keys. Nothing is renamed, nothing is reordered, and the terminal
    // stays terminal.
    serial_println!(
        "[wc-h] rollup win={} scope={} emit={} age_ms={} pop=budgeted samples={} budget={} pop=all-presents torn={} declines={} fixture={} whole={} banded={} lines={} minspan={} minspan_bytes={} maxpresent_us={} pop=constant frame_us={} -> {}",
        id,
        scope,
        emit,
        age_ms,
        taken.min(SAMPLES),
        SAMPLES,
        torn_n,
        decl_n,
        H_FIXTURE[i].load(Ordering::Relaxed),
        H_WHOLE[i].load(Ordering::Relaxed),
        H_BANDED[i].load(Ordering::Relaxed),
        H_LINES[i].load(Ordering::Relaxed),
        minspan,
        minbytes,
        H_MAXPRES[i].load(Ordering::Relaxed),
        FRAME_US,
        verdict
    );
    // Re-arm the refresh from AFTER the serial write, so `CENSUS_PERIOD_US` bounds the duty cycle
    // this instrument imposes on the composite path and not merely the gap between line starts. Both
    // stores happen on every emission — including the two latched ones — so the first refresh is
    // measured from the window's first rollup rather than from boot, and a window whose first rollup
    // is its last activity never refreshes at all.
    H_LASTCENSUS[i].store(census_total(i), Ordering::Relaxed);
    H_LASTROLL[i].store(now_cycles(), Ordering::Relaxed);
}

// ---- WC-K — the staged DESKTOP FILL's own witness ----------------------------------------------

/// Staged erase fills to report per boot. The erase path has no window id to budget against — an
/// erase belongs to a box that no longer has an owner — so the budget is global, and four is the
/// same figure the per-window witnesses use for the same reason: enough to tell a steady state from
/// a one-off, few enough that the serial writes do not perturb a close path.
const E_SAMPLES: u32 = 4;

/// Deferral lines to print before going quiet. Deliberately LARGER than [`E_SAMPLES`], and
/// deliberately NOT the same budget.
///
/// WC-H's `stage_decline` shares one budget between successes and declines, which leaves a real
/// blind spot: a boot that stages its first four composites and declines every one thereafter stops
/// printing, and the rollup that already fired cannot retract. That is survivable there because a
/// window composites continuously and the counters keep moving. It is not survivable here, because
/// what a deferral reports is a fill the panel has NOT received yet, and a boot that starts
/// deferring only after the rollup has printed must still be visible on the wire. So a deferral is
/// printed whether or not the sample budget is spent, and only a boot that defers pathologically
/// (more than this many) goes quiet, with the counters still running.
const E_DECL_LINES: u32 = 16;

/// Staged fills recorded so far, capped at [`E_SAMPLES`] for line purposes.
static E_TAKEN: AtomicU32 = AtomicU32::new(0);
/// Staged fills whose PRESENT phase alone outran the beam's time on the box.
static E_TORN: AtomicU32 = AtomicU32::new(0);
/// Staged fills whose present was not `h` contiguous, row-stepped runs. Structural, not timing.
static E_NONCONTIG: AtomicU32 = AtomicU32::new(0);
/// Fills the panel never received.
///
/// Under WC-K this meant "fell back to the direct `fill_rect`" — the tearing regime. WC-L removed
/// that fallback, so the count now means the strictly worse and strictly rarer thing: a fill that
/// could neither stage NOR be deferred, because its decline reason (`geom`, `cap`) is permanent for
/// that box and requeueing it would spin the drain forever. The `-> UNSTAGED` verdict this drives is
/// therefore still exactly right: those panel rows hold whatever the departed window left there, and
/// no later pass is going to fix them. Neither reason is reachable on any panel this kernel drives.
static E_DECLINE: AtomicU32 = AtomicU32::new(0);
/// WC-L — fills that could not reach the back layer on their first attempt and were queued as
/// deferred damage instead of written direct. Not a defect: the fill still arrives through the
/// staged path, one composite pass later.
static E_DEFER: AtomicU32 = AtomicU32::new(0);
/// WC-L — deferred boxes that were still unable to stage when the drain retried them, and went back
/// on the queue. A steady-state non-zero here means the staging lock is contended for longer than a
/// composite interval, which is worth seeing even though the outcome is still tear-free.
static E_REDEFER: AtomicU32 = AtomicU32::new(0);

/// WC-L — how many requeues a boot may accumulate before the rollup calls the erase path STARVED.
///
/// A deferral that is delivered is a latency cost and nothing more. A deferral that keeps being
/// requeued is a repaint that has NOT happened, and on the panel that is a dead window's last frame
/// sitting where the desktop should be — the P61 ghost, arrived by a new route. Without a threshold
/// the rollup would print `TEAR-FREE` over exactly that, which is the failure mode WC-K had (a
/// verdict that described the samples it liked rather than the panel).
///
/// Eight is `MAX_DEFER`: a full queue's worth of boxes each missing one drain is the largest requeue
/// count a single transient contention window can honestly produce. Anything beyond it is either a
/// second contention window or a box that is not making progress, and both deserve the operator's
/// attention. Deliberately a low bar — this verdict is meant to be sensitive, because the thing it
/// guards against is invisible in every other field on the line.
const E_REDEFER_MAX: u32 = 8;
/// WC-L — deferred boxes absorbed into an existing queue entry's bounding box because the queue was
/// full. Sound (the drain re-damages every window the enlarged box reaches, exactly as WC-J's
/// `reclaim` does), but it repaints more panel than strictly owed, so it is counted.
static E_COALESCE: AtomicU32 = AtomicU32::new(0);
/// Deferral lines emitted, capped at [`E_DECL_LINES`].
static E_DECL_LINES_OUT: AtomicU32 = AtomicU32::new(0);
/// The largest present-phase duration seen across staged fills, in microseconds.
static E_MAXPRES: AtomicU64 = AtomicU64::new(0);
/// Total panel rows presented by staged fills — the size of what this discipline now covers.
static E_ROWS: AtomicU64 = AtomicU64::new(0);

/// WC-L — record a desktop fill that could not reach the back layer on this attempt and was queued
/// as DEFERRED DAMAGE rather than written straight to the front buffer.
///
/// This replaces WC-K's `erase_decline`, and the replacement is the whole of WC-L. WC-K staged the
/// erase but kept a direct `fill_rect` as the last resort, so under real contention — the P64
/// attended boot, two focus tab-cycle transitions at ~99% core load — the erase path took
/// `reason=lock` and wrote the desktop fill DIRECTLY into the buffer the HVS was scanning. That is
/// the exact writing shape WC-G convicted and WC-K existed to remove, so the fallback did not make
/// the arc robust, it made the arc conditional. There is now no direct fill to fall back to: the box
/// goes on [`super::wm`]'s deferred-erase queue and the next composite pass erases it through the
/// staged path. One frame late is a cost; a direct front-buffer write is a defect.
///
/// The line deliberately says `staged=defer`, not `staged=no`. The spec FORBIDs `staged=no` and
/// `-> DIRECT` because both name the tearing regime; a deferral is neither, and giving it the
/// forbidden vocabulary would either fail honest boots or force the FORBID to be loosened — and the
/// FORBID is the arc's verdict.
///
/// The reasons are [`super::wm::stage_window`]'s, reused rather than duplicated: the two staging
/// paths defer for the same four causes and a separate vocabulary would only invite the two to
/// drift.
pub fn erase_defer(w: usize, h: usize, reason: u32, requeued: bool) {
    E_DEFER.fetch_add(1, Ordering::Relaxed);
    if requeued {
        let r = E_REDEFER.fetch_add(1, Ordering::Relaxed) + 1;
        // The STARVED verdict has to be reachable for the WHOLE boot, not only until the sample
        // rollup fires. The rollup prints once, at sample `E_SAMPLES`, and starvation by its nature
        // arrives LATE — the contention that causes it is a loaded, long-running desktop, not a
        // boot's first four fills. A rollup that has already printed cannot retract, so the
        // threshold gets its own one-shot line here, on the same reasoning that makes the deferral
        // lines unbudgeted: the FORBID is only worth having if the boot can still trip it.
        if r == E_REDEFER_MAX + 1 {
            serial_println!(
                "[wc-k] rollup scope=starve redefers={} limit={} -> STARVED",
                r,
                E_REDEFER_MAX
            );
        }
    }
    let n = E_DECL_LINES_OUT.fetch_add(1, Ordering::Relaxed) + 1;
    if n > E_DECL_LINES {
        E_DECL_LINES_OUT.store(E_DECL_LINES + 1, Ordering::Relaxed);
        return;
    }
    serial_println!(
        "[wc-k] erase box={}x{} staged=defer reason={} requeued={} -> DEFERRED",
        w,
        h,
        decl_name(reason),
        if requeued { "yes" } else { "no" }
    );
}

/// WC-L — record a desktop fill that was DROPPED: it could not stage, and its reason is permanent
/// for that box, so deferring it would put work on the queue that can never come off. The panel rows
/// keep whatever was there. This is a defect, and it is meant to read as one — it feeds `declines=`
/// and so the rollup's `-> UNSTAGED`, which the spec FORBIDs.
///
/// Shares [`E_DECL_LINES`]'s budget with the deferral lines: both describe a fill that did not
/// arrive, and a boot producing enough of either to exhaust the budget has already said so.
pub fn erase_drop(w: usize, h: usize, reason: u32) {
    E_DECLINE.fetch_add(1, Ordering::Relaxed);
    let n = E_DECL_LINES_OUT.fetch_add(1, Ordering::Relaxed) + 1;
    if n > E_DECL_LINES {
        E_DECL_LINES_OUT.store(E_DECL_LINES + 1, Ordering::Relaxed);
        return;
    }
    serial_println!(
        "[wc-k] erase box={}x{} staged=drop reason={} -> LOST",
        w,
        h,
        decl_name(reason)
    );
}

/// WC-L — record a deferred box absorbed into an existing queue entry because the queue was full.
/// Counted rather than printed per event: coalescing is bounded work on a bounded queue, and the
/// rollup's `coalesced=` is where a boot that is doing it constantly becomes visible.
pub fn erase_coalesce() {
    E_COALESCE.fetch_add(1, Ordering::Relaxed);
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
        // `-> DIRECT`, which needs no completeness claim and which — after WC-L — no emitter in this
        // module can produce at all.
        //
        // WC-L: `defers` and `coalesced` deliberately do NOT enter the precedence. A deferral that is
        // delivered arrived through the staged path one pass late, so it neither tore nor went
        // direct, and demoting the verdict for it would make the honest report of contention
        // indistinguishable from the regime the arc removed.
        //
        // `redefers` is different, and the lens review was right to insist on it. A requeue is a
        // repaint that has not happened yet, and past `E_REDEFER_MAX` the honest reading is that the
        // erase path is not draining — which on the panel is a dead window's frame where the desktop
        // should be. `TEAR-FREE` over a visible ghost would be WC-K's mistake repeated in a new
        // place. It sits BELOW `UNSTAGED` in the precedence because a starved box may still arrive
        // (nothing has been lost, only delayed), where a dropped one provably never will.
        let torn_n = E_TORN.load(Ordering::Relaxed);
        let nc = E_NONCONTIG.load(Ordering::Relaxed);
        let decl = E_DECLINE.load(Ordering::Relaxed);
        let redef = E_REDEFER.load(Ordering::Relaxed);
        let verdict = if nc > 0 {
            "SPLIT"
        } else if torn_n > 0 {
            "AT-RISK"
        } else if decl > 0 {
            "UNSTAGED"
        } else if redef > E_REDEFER_MAX {
            "STARVED"
        } else {
            "TEAR-FREE"
        };
        serial_println!(
            "[wc-k] rollup scope=fills samples={} rows={} torn={} noncontig={} declines={} defers={} redefers={} coalesced={} maxpresent_us={} frame_us={} -> {}",
            n,
            E_ROWS.load(Ordering::Relaxed),
            torn_n,
            nc,
            decl,
            E_DEFER.load(Ordering::Relaxed),
            E_REDEFER.load(Ordering::Relaxed),
            E_COALESCE.load(Ordering::Relaxed),
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

/// The coherency leg's cache maintenance over the source surface.
///
/// On aarch64 this is the `DC CIVAC` sweep the module note argues for. On x86 the compositor, the
/// owner and the scan-out all read the same coherent view — the same reason
/// [`super::FrameBuffer::flush_range`] delegates to an arch hook that is a no-op there — so there is
/// nothing to maintain and the leg reduces to reading the same bytes twice. `blit == civac` is then
/// true by construction rather than by measurement, which is the honest reading of the counter on a
/// coherent target: it can still catch a genuinely non-cacheable or mismatched alias, because such a
/// mapping would diverge between the two reads for reasons the cache op never enters into.
///
/// The call site is identical on both arches by design — the ordering note in [`begin`] about where
/// the clock starts relative to this call is load-bearing and must not acquire an arch split.
#[inline]
fn clean_invalidate_surface(addr: usize, len: usize) {
    #[cfg(target_arch = "aarch64")]
    crate::arch::cache::clean_invalidate_range(addr, len);
    #[cfg(not(target_arch = "aarch64"))]
    let _ = (addr, len);
}

/// CNTVCT_EL0 ticks converted to microseconds at the generic timer's own rate. `CNTFRQ_EL0` is read
/// per call (a system register read, cheap next to a 64 KiB checksum) and falls back to the Pi 4's
/// 54 MHz if the firmware left it zero, so a bad `CNTFRQ` degrades the number rather than dividing
/// by zero.
#[cfg(target_arch = "aarch64")]
pub(super) fn cycles_to_us(dt: u64) -> u64 {
    let frq: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) frq, options(nomem, nostack, preserves_flags));
    }
    let frq = if frq == 0 { 54_000_000 } else { frq };
    dt.saturating_mul(1_000_000) / frq
}

/// `rdtsc` ticks converted to microseconds at the rate `apic::calibrate` measured against the ACPI
/// PM timer. Same contract as the aarch64 reader: read the rate per call, and degrade rather than
/// divide by zero if it is unavailable.
///
/// The uncalibrated fallback is deliberately the rate [`crate::arch::HW_WAIT_BUDGET`] already
/// assumes for an uncalibrated TSC (1.25 GHz — that budget's 2.5e9 cycles over its 2 s), so the two
/// consumers of an unknown TSC rate in this kernel guess the same number. An uncalibrated reading is
/// wrong by whatever the real part's ratio to that figure is, in either direction, so a `torn=` or
/// `slow=` verdict taken before calibration would not be evidence of anything — calibration happens
/// long before any window composites, so no sample this witness prints is in that regime.
#[cfg(target_arch = "x86_64")]
pub(super) fn cycles_to_us(dt: u64) -> u64 {
    let hz = crate::arch::apic::tsc_hz();
    let hz = if hz == 0 { 1_250_000_000 } else { hz };
    dt.saturating_mul(1_000_000) / hz
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
    /// WC-G/M1 — microseconds the `blit` checksum took: one full-surface volatile read.
    cks_blit_us: u64,
    /// WC-G/M1 — microseconds the coherency leg took: [`clean_invalidate_surface`] plus the `civac`
    /// re-read, bracketed TOGETHER and deliberately so. On aarch64 the cache op and the refill it
    /// forces are one cost with one cause; splitting them would credit the sweep with ~nothing and
    /// charge the refill to a read that is only expensive because the sweep dropped the lines. On
    /// x86 the op is a no-op and this reduces to the second full-surface read, which is the honest
    /// reading there — see [`clean_invalidate_surface`].
    civac_us: u64,
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
    // WC-G/M1 — phase timestamps around the EXISTING operations, all of them BEFORE `t0` is set.
    // Four clock reads on a path that already does two full-surface volatile reads is arithmetic
    // next to the work being measured, and — this is the load-bearing part — none of it lands
    // between the `t0` assignment and the return, so the ordering law below is untouched and `us=`
    // still contains the copy and nothing else.
    let tp0 = now_cycles();
    let cks_blit = checksum(surf, surf_len);
    let tp1 = now_cycles();
    // The coherency leg. See the module note on why this cleans as well as invalidates.
    clean_invalidate_surface(surf, surf_len);
    let cks_civac = checksum(surf, surf_len);
    let tp2 = now_cycles();
    let cks_blit_us = cycles_to_us(tp1.saturating_sub(tp0));
    let civac_us = cycles_to_us(tp2.saturating_sub(tp1));

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
        cks_blit_us,
        civac_us,
        // LOAD-BEARING ORDERING: the clock starts AFTER the `civac` re-read, and must stay there.
        // `clean_invalidate_range` drops every line of the surface, and the `cks_civac` checksum
        // immediately re-reads all of it — which is precisely what re-warms the source for the blit
        // that follows. Start `t0` before that re-read (say, alongside `cks_blit`) and `us=` silently
        // absorbs a full 64 KiB cache refill that the uninstrumented path never pays, inflating the
        // very number the timing verdict rests on. The measured interval must contain the copy and
        // nothing else.
        //
        // The law has a second half, which WC-G/M1's phase timings are the first thing to test:
        // NOTHING may be inserted between this assignment and the return. It is the last field of
        // the literal for that reason, so anything added to this function lands above it and is
        // paid before the clock starts. `cks_blit_us` and `civac_us` are taken that way — they
        // bracket the operations already there and do not move `t0`.
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
    // WC-G/M1 — `us=` above is computed FIRST and from `p.t0` alone, exactly as it always was. Every
    // phase clock below starts after it, so no profiling read can enter the timing verdict's bracket.
    let tp0 = now_cycles();
    let cks_after = checksum(p.surf, p.surf_len);
    let tp1 = now_cycles();
    let cks_after_us = cycles_to_us(tp1.saturating_sub(tp0));

    // Re-derive the destination from the source, one probe per SOURCE pixel (the top-left
    // destination pixel of each upscale cell). Bounds mirror `draw_window`'s: the panel clip, the
    // stride column bound, and the `surf_len` row bound.
    let info = fb.info();
    let (pw, ph) = (info.width, info.height);
    let mut checked = 0usize;
    let mut bad = 0usize;
    // WC-G/M1 — the read-back's own bracket. On x86 the glass is WC-mapped PCIe memory and every
    // `read_pixel` is an uncached round trip to the device, one per SOURCE pixel, so this loop is
    // the phase most likely to dominate the pass — which is exactly why it could not go on being
    // reported as part of an undifferentiated total. The bracket wraps the loop and nothing else:
    // `checked` and `bad` are declared above it and read below it, so the clock contains no
    // formatting and no serial.
    let tp2 = now_cycles();
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
    let readback_us = cycles_to_us(now_cycles().saturating_sub(tp2));

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

    // WC-G/M1 — WHERE THE PASS ACTUALLY GOES.
    //
    // Everything above this point measures the COPY and its correctness, and does so by excluding
    // the witness's own work from every bracket. That exclusion is right, and it is also why the
    // instrument's cost has never appeared on any line it prints. On metal it is not small: an
    // armed x86 boot spends ~2.87 s per instrumented pass, four passes, ~11.5 s of a 17.3 s block
    // whose real GPU work is ~1.4 s. "The witness is expensive" is not an actionable statement; a
    // per-phase decomposition is, and this line is that decomposition and nothing more. It draws no
    // verdict and carries no threshold — reshaping the pass is a later decision that needs this
    // measurement first.
    //
    // The four phases are the four things a pass does besides copying: two full-surface checksums
    // before the blit (`cks_blit_us`, and `civac_us` for the cache op plus its re-read), one after
    // (`cks_after_us`), and the per-source-pixel read-back against the glass (`readback_us`, with
    // `probes=` the pixels it actually reached, so a rate can be derived rather than assumed).
    // `surf_bytes=` is the size all three checksums walked, which is what makes the three of them
    // comparable across windows of different sizes.
    //
    // ### Its own cost, stated rather than hidden
    //
    // ~150 bytes on the wire. At 115200 baud with the usual 10 bits per byte that is ~13 ms of a
    // core's time per sampled pass — real, and charged to the composite path exactly like the
    // sample line above it. It is OUTSIDE every bracket on this line and outside `us=`, by the same
    // rule that puts `stage_flush`'s print after `wcg::end`: an instrument that inflates the number
    // it appears next to is worse than no instrument. So the honest accounting is that a sampled
    // pass now costs ~13 ms more wall time than it did and reports the same measurements it would
    // have reported without the line. Against the ~2.87 s pass this decomposes, that is ~0.45%.
    //
    // ### Why it is a separate line, and why it needs no budget of its own
    //
    // Separate because the sample line's key order is load-bearing across seats — the pi4 spec
    // matches `\[wc-g\] win=.* fbbad=.* slow=.* ->` with the verdict terminal — and four more keys
    // in front of that terminal would be four more chances to break a gate on another platform for
    // a field that platform does not read. No budget because there is nothing to budget: `end` is
    // reachable only with a `Probe`, and `begin` hands one out only while [`SAMPLES`] is unspent.
    // This line therefore rides the sample line's budget exactly, one for one, and adds no
    // unbudgeted serial anywhere in the boot.
    //
    // It leads with `[wc-g] ` deliberately: `fbcon`'s `PANEL_MUTE_TAGS` mutes serial tags from the
    // panel by that prefix, so an x86 witness build keeps this line off the glass it is measuring.
    serial_println!(
        "[wc-g] prof win={} seq={} surf_bytes={} cks_blit_us={} civac_us={} cks_after_us={} probes={} readback_us={}",
        p.id,
        p.seq,
        p.surf_len,
        p.cks_blit_us,
        p.civac_us,
        cks_after_us,
        checked,
        readback_us
    );
    // The per-window ledger the rollup's `wit_us=` reports. Accumulated AFTER the prints, so a
    // window's total is the sum of the four measured phases and carries no serial time. See
    // [`W_WITUS`].
    W_WITUS[w].fetch_add(
        p.cks_blit_us
            .saturating_add(p.civac_us)
            .saturating_add(cks_after_us)
            .saturating_add(readback_us),
        Ordering::Relaxed,
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
        // WC-G/M1 — `wit_us=` is an INSERTION between `maxus=` and `frame_us=`, and that placement is
        // the whole of what the spec permits here. The pi4 gate matches
        // `\[wc-g\] rollup win=.* scope=window .*frame_us=.* ->`, so fields may be inserted between
        // matched keys and nothing may be renamed, reordered, or moved past the terminal verdict. It
        // sits beside `maxus=` because the two are the natural pair to read together: the longest
        // single copy this window did, and the total this window's four samples spent measuring.
        //
        // It is a per-WINDOW total over SAMPLED passes — the same population `samples=` counts — and
        // it is not a rate, a maximum, or a boot figure. Divide by `samples=` for a per-pass mean;
        // the per-phase split is on the `prof` lines above.
        serial_println!(
            "[wc-g] rollup win={} scope=window samples={} coher={} race={} blit={} clean={} slow={} maxus={} wit_us={} frame_us={} -> {}",
            p.id,
            n,
            coher,
            race,
            blitn,
            clean,
            slown,
            W_MAXUS[w].load(Ordering::Relaxed),
            W_WITUS[w].load(Ordering::Relaxed),
            FRAME_US,
            dominant
        );
    }
}
