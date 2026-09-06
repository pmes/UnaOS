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
//!
//! ## WC-G/M3 — what the battery costs, and paying for it as it is used
//!
//! M1 decomposed the pass and named the phase: on x86 an armed boot spends ~2.87 s per instrumented
//! `[wc-g]` pass and ~11.5 s of a 17.3 s block on four of them, against ~1.4 s of real GPU work, and
//! the glass read-back is where it goes. The reason is the target rather than the loop — the Kepler
//! framebuffer is write-combining PCIe memory, so each `read_pixel` is three uncached round trips to
//! the device and there is one `read_pixel` per source pixel. M3 does two things about that, and only
//! one of them is a coverage decision.
//!
//! **The read shape, on x86 and unconditionally.** [`readback`] now walks a destination row through
//! [`GlassRow`], which reads the aperture in aligned 64-bit words and holds the last word so that
//! adjacent probes share a transaction. Three byte trips per pixel become one word trip per pixel,
//! or per two pixels where the probes are contiguous. The SAME probe set is evaluated against the
//! same bytes, with `read_pixel`'s bounds and colour decode reproduced exactly and delegated to
//! wherever the wide path's preconditions do not hold — so `fbbad`, `checked` and every verdict are
//! unchanged. Nothing about what this witness can catch moves, which is why it carries no knob.
//! aarch64 keeps the per-probe `read_pixel` call it has always made.
//!
//! **The battery's shape, under `wcg-paygo`.** Knob off, the four passes are what they were and
//! every line is byte-identical, on either arch. Knob on, a window's battery is paid as the desktop
//! uses it rather than as the boot starts it:
//!
//! ARCH-PARITY (rmbp-7) — this used to read "and on x86 only", and the `#[cfg]`s enforced it: all
//! 79 paygo gates were `all(target_arch = "x86_64", feature = "wcg-paygo")`. The arch term bought
//! nothing. Every symbol the policy reaches is defined for both chips — [`crate::arch::now_cycles`],
//! this module's own two [`cycles_to_us`], [`crate::bootpace::origin_cycles`]/`origin_hz`,
//! [`checksum`], and `wm::OccSnap` — so what the term forbade was not an unimplementable behaviour
//! but a REACHABLE one. It is now `feature = "wcg-paygo"` alone. That arms nothing: no aarch64 check
//! leg and no Pi or Orin media set names the knob today, so the aarch64 build folds the whole family
//! away exactly as the arch term made it fold away, and the wire below is unmoved. What changes is
//! that a Pi arc can now ARM the deferral instead of having to port it first.
//!
//! | pass | coverage | when | on the wire |
//! |------|----------|------|-------------|
//! | 1 | LATTICE — every [`PAYGO_LATTICE_N`]th source pixel per row, phase rotating one column per row | immediately, as before | `coverage=lattice16` |
//! | 2..[`SAMPLES`] | FULL — every source pixel | once [`since_entry_ms`] passes [`PAYGO_DEFER_MS`] | `coverage=full` |
//!
//! The lattice COLLAPSES to full coverage on a rect narrower than its own step — see [`readback`] —
//! and the marker is derived from the step the walk actually used, so a narrow window says
//! `coverage=full` rather than claiming a sampling it did not perform.
//!
//! ### What a sampled pass catches, and what it cannot
//!
//! Stating this is the point of the marker, so it is stated here in full rather than implied by the
//! step. A lattice pass CATCHES:
//!
//! - **band and smear-class defects** — the defect class WC-G was built for and the shape in the
//!   photograph. Every row is probed at every step, so a full-row band cannot be missed at all.
//! - **any horizontal garble run of [`PAYGO_LATTICE_N`] pixels or more**, in any row, deterministically:
//!   a run that wide contains a probe whatever the row's phase.
//! - **stride and geometry faults** — a wrong row step, pitch or origin displaces the whole surface,
//!   which no probe in any row agrees with.
//! - **a one-pixel-wide vertical defect**, within [`PAYGO_LATTICE_N`] rows: the phase rotates by one
//!   column per row, so every column is probed once over any `PAYGO_LATTICE_N` consecutive rows.
//!
//! It CANNOT catch an **isolated single-pixel blit error** — one wrong pixel, in one row, at a column
//! that row's phase does not visit. That is the whole of the narrowing, and it is why full coverage
//! is deferred rather than dropped: passes 2..4 probe every pixel, so the battery still ends with the
//! coverage it always had, on a live desktop instead of inside the boot burst.
//!
//! ### Nothing stops counting, and nothing goes quiet
//!
//! WC-H2's law governs here too, and each half of it is discharged by a specific field:
//!
//! - A sampled pass is marked ON ITS OWN LINE. `checked` was always the honest denominator, but a
//!   denominator does not say why it is small; `coverage=` does, and a knob-on build marks the full
//!   passes too so the marker's absence is never the thing carrying the meaning. No `fbbad=0/…` in
//!   this module ever reads as a clearance it did not earn.
//! - A deferred pass is a pass DECLINED, not a counter capped. [`PAYGO_DEFERRED`] counts every
//!   declined blit, unbudgeted, and the window's `[wc-g] paygo … state=waiting … -> DEFERRED` line is
//!   RE-EMITTED on [`CENSUS_PERIOD_US`] cadence with an `emit=` ordinal — so `deferred=` is a running
//!   census with its moment stamped (`since_entry_ms=`), not a figure frozen at the instant the
//!   deferral began. A one-shot there would have been [`H_TORN`]'s convicted shape exactly: printed
//!   once, at first decline, where the count is 1 by construction. The reader's rule is the module's
//!   standing one — greatest `emit=` per `win=` wins, and these lines are never summed. The terminal
//!   `state=complete … -> PAID` carries the same census, and the rollup carries `paygo=yes` so the
//!   verdict names the policy it was drawn under.
//! - The budget is not spent by a decline, so a window that stops presenting keeps its remaining
//!   samples unspent. That is not a new behaviour to reason about: it is exactly what this module
//!   already does with a window that stops compositing.
//!
//! Every FORBID stays armed for the whole boot either way. The checksum legs — `app`, `blit`,
//! `civac`, `after` — are cacheable RAM reads, cheap next to the glass, and run in full on EVERY
//! sampled pass unchanged. Only the read-back is what sampling and deferral reshape.
//!
//! ### What deferral costs somewhere else, stated because it is visible
//!
//! A spent probe declines the cursor sprite's overlay for that composite pass — `video::wm` sets
//! `may_overlay = false` whenever `wcg::begin` hands out a `Probe`, because this witness reads its
//! destination pixels back and a cursor composited into them would read as a blit defect. Knob off,
//! all four of those passes happen during the boot burst, before anyone is pointing at anything.
//! Knob on, three of the four move onto the LIVE DESKTOP, so three composite passes that would have
//! carried the sprite no longer do. That is a visible-artifact class — a brief cursor drop — and it is
//! named here rather than discovered on the bench. It is not a correctness loss: CURSOR-4 already
//! rules the exclusion conservative and coverage-safe, the passes are budgeted and few, and the tail
//! repaints. It is a cost that MOVED with the samples, not one this arc introduced.
//!
//! ### Who actually reads these lines
//!
//! Worth stating, because the key-order discipline above reads as though several gates depended on
//! it: **no x86 spec reads any `[wc-g]` line.** `unaos/scripts/specs/pi4-regression.spec` is the only
//! automated reader this module has, on either arch — two REQUIREs and three FORBIDs. Every
//! insertion rule here therefore protects the PI4 track, which is precisely why a knob-gated
//! feature is held to it: the file is shared, the aarch64 wire must not move, and the one gate that
//! would notice is on the other platform's bench. A field added here is checked against that spec
//! because nothing on this side would catch the break.
//!
//! ARCH-PARITY (rmbp-7) — that rule got STRICTER, not looser, when the paygo family lost its arch
//! term. Before, a `[wc-g]` field could only reach the pi4 spec by someone editing an unconditional
//! line; now it can also reach it by someone adding `wcg-paygo` to an aarch64 leg. The knob is the
//! whole of the protection, and it is why the port stopped at the `#[cfg]`s: no leg, no `K8_FEATS`
//! and no `arm_features()` line was touched, so the set of builds that reach these lines is exactly
//! what it was. Arming the knob on aarch64 is a decision for the Pi track to take against that spec,
//! with the two `arroyo` changes it implies — a type-check leg that covers the newly-live aarch64
//! half, and a ruling on whether the knob joins the media strip list.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use super::FrameBuffer;
use crate::arch::now_cycles;
/// WC-G/M3 — the wide glass read-back decodes the panel's colour order itself, where
/// [`super::FrameBuffer::read_pixel`] decodes it per pixel. x86 only, like the path that uses it.
#[cfg(target_arch = "x86_64")]
use unaos_boot_info::PixelFormat;

/// Instrumented blits per window id. Four is enough to distinguish a steady state from a one-off
/// and small enough that the checksum reads do not dominate the interval being timed.
const SAMPLES: u32 = 4;

/// Window ids this witness tracks — wm ids run 1..=MAX_WINDOWS and this table indexes them raw,
/// so it needs MAX_WINDOWS+1 rows (index 0 is dead). Derived, with a tripwire: the headroom
/// review caught the literal 8 leaving WC-G silently blind on 5 of 12 rows after the raise.
const IDS: usize = crate::video::wm::MAX_WINDOWS + 1;
const _: () = assert!(IDS > crate::video::wm::MAX_WINDOWS);

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
///
/// WCGSEAM-HB — REFUNDED passes are included: a refunded sample's four phases were paid in full
/// before the refund decision existed, so the ledger charges them exactly as it charges an
/// adjudicated pass. `wit_us=` therefore reads "what the instrument cost this window", not "what
/// the samples= population cost", whenever `refunded=` lines precede the rollup.
static W_WITUS: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];

// ---- WCGSEAM — the boot-seam writer census (a DISCRIMINATOR, not a remedy) ---------------------
//
// The standing `[wc-g] win=1 -> COHER`/`RACE-BLIT` red on armed bench-geometry boots is attributed
// to a BOOT-SEAM CONCURRENT WRITER: fbcon's glyph raster paints the routed console window's surface
// from print context while the compositor's checksum bracket reads it. WCGWIN1 (PARITY §6.13)
// proved LIVECON is NOT the fix — the writer fires before the deferral is armed — and named two
// remedy candidates. This census is the instrument that must precede either: it captures WHO the
// concurrent writer was at the moment a sample convicts, read-only, lock-free, and printed from the
// same compositor context as the `[wc-g]` line itself (never from print context — a serial write
// inside the glyph path would recurse into `_print`).
//
// fbcon charges the census at its two paint sites (the classic per-byte path under the FBCON lock,
// and x86's unlocked split path) whenever the glyphs landed in the ROUTED window surface — three
// relaxed atomic stores, no lock, no branch on the render or input path. `begin` snapshots the
// counter above `t0` (one relaxed load, paid before the clock starts, per the ordering law on the
// `Probe` literal); a convicting `end` reads it again and prints `[wcgseam]` with the bracket delta.
//
// THE CENSUS HAS SPOKEN (exec-wcg, 2026-08-25, QEMU raspi4b at bench geometry —
// `UNAOS_PIDESK=1 UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 150`, PARITY §6.14's own
// A/B configuration). All three convictions of the armed boot carried `-> GLYPH-RASTER` with
// `delta=149/216/325` and `routed=yes` — the writer caught INSIDE the bracket, every time; the
// pre-registered exoneration branch (`delta=0` with a large age) did not occur. The attribution
// above is CONFIRMED, not artefact: fbcon's glyph raster is the concurrent writer. `locked=`
// tracked the census total exactly — on aarch64 the split path does not exist, so 100% of the
// writers take the FBCON lock, which per the pre-registered reading makes remedy (a) sufficient
// in REACH and leaves the choice against it a question of COST (an FBCON spinlock held across a
// ~10-29 ms bracket, waited on from print context with interrupts masked). The remedy taken is
// neither (a) nor (b) but the granting seats' preferred shape — the HONEST BRACKET below
// (WCGSEAM-HB): a convicting sample whose full adjudication span the census marks dirty is
// REFUNDED and re-armed rather than adjudicated, bounded by [`REARM_MAX`]; a quiet-bracket
// conviction still convicts, so every spec FORBID keeps its teeth. x86 QEMU cannot fire any of
// this at runtime (`:: kepler: no-device ::` — no takeover, no routed console, `SEAM_WIN` never
// written); x86 metal has never convicted (x86-witness.spec: zero hits, boots 7/8).

/// WCGSEAM — glyph-raster paint batches fbcon has landed in the ROUTED console-window surface, for
/// the whole boot. One count per `write_byte` on the classic path (a glyph or a newline's fills),
/// one per painted chunk on the split path — a census of write EVENTS, not bytes.
static SEAM_WRITES: AtomicU64 = AtomicU64::new(0);
/// WCGSEAM — [`crate::arch::now_cycles`] at the most recent routed glyph write. What `last_age_us=`
/// is derived from: a conviction with `delta=0` but a small age says the store landed just before
/// the bracket opened — the COHER shape — rather than inside it.
static SEAM_LAST_CYC: AtomicU64 = AtomicU64::new(0);
/// WCGSEAM — the split of [`SEAM_WRITES`] painted UNDER the FBCON lock (the classic per-byte path,
/// print context, interrupts masked). The remainder is the unlocked split path. Which side the
/// writer is on is exactly what separates remedy (a) FBCON-lock the console blit — which can only
/// serialise writers that take the lock — from remedy (b) decline win=1 in `begin` while routed.
static SEAM_LOCKED: AtomicU64 = AtomicU64::new(0);
/// WCGSEAM — the routed console's window id as fbcon last charged it, `wm::WIN_NONE` (0) until the
/// first routed glyph write. The `[wcgseam]` line prints only for THIS window: a conviction on an
/// app window has nothing to learn from a console census.
static SEAM_WIN: AtomicU32 = AtomicU32::new(0);

/// WCGSEAM-HB — per-id: samples this window has REFUNDED under the honest bracket (see the refund
/// block in [`end`]). Deliberately NOT reset by [`wch_recycle`], by exactly `TAKEN`'s rule: the cap
/// is a per-boot serial/time bound, and a recycle that re-armed it would unbound both. Compiled out
/// wherever the refund is (the x86 `wcg-paygo` chunk machinery banks part-paid samples, and a
/// refund mid-battery would desync the paygo ledger — on that build a dirty-bracket conviction
/// adjudicates as before, with the `[wcgseam]` line beside it).
#[cfg(not(all(target_arch = "x86_64", feature = "wcg-paygo")))]
static W_REARM: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];

/// WCGSEAM-HB — how many dirty-bracket convictions a window may refund before the instrument
/// adjudicates anyway. The bound is what keeps the four spec FORBIDs live even against a writer
/// that NEVER quiets (the `livecon` steady state §6.14 left unmeasured): 16 refunds at the bench
/// geometry's ~43 ms per pass is ≤ ~0.7 s of witness time and ≤ ~2.4 KB of `[wcgseam]` serial,
/// and then convictions resume. The boot-seam writer this remedy excuses goes quiet at
/// `fbcon::detach()`, well inside the bound on every capture read so far.
#[cfg(not(all(target_arch = "x86_64", feature = "wcg-paygo")))]
const REARM_MAX: u32 = 16;

/// WCGSEAM — fbcon's charge point: one routed glyph-raster write event landed in the console
/// window's surface. `locked` says the FBCON lock was held (classic path) or not (split path);
/// `win` is [`super::fbcon`]'s `CONSOLE_WIN` at the moment of the write.
///
/// Three relaxed atomics and nothing else — called from print context, so it must never take a
/// lock, allocate, or print. Budget-free deliberately: the census is the DENOMINATOR a conviction
/// is read against, and a capped count would go dark exactly when the boot is chatty enough to
/// matter.
pub fn seam_glyph_note(locked: bool, win: u32) {
    SEAM_WRITES.fetch_add(1, Ordering::Relaxed);
    if locked {
        SEAM_LOCKED.fetch_add(1, Ordering::Relaxed);
    }
    SEAM_LAST_CYC.store(now_cycles(), Ordering::Relaxed);
    if win != 0 {
        SEAM_WIN.store(win, Ordering::Relaxed);
    }
}

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
/// WCH-SPREAD — per-id: the SMALLEST present-phase duration seen, in microseconds. The floor that
/// makes [`H_MAXPRES`] readable: a maximum with no minimum beside it cannot say whether the window's
/// presents are uniformly expensive or wildly uneven, and those are different faults. `u64::MAX` until
/// the window's first present, which [`stage_rollup`] prints as `0`.
static H_MINPRES: [AtomicU64; IDS] = [const { AtomicU64::new(u64::MAX) }; IDS];
/// WCH-SPREAD — per-id: the extremes of the present phase's INVERSE THROUGHPUT, in nanoseconds per
/// 4 KiB copied. Their ratio is the rollup's `presspread=`.
///
/// **Why a rate and not the raw duration.** A present's honest cost is proportional to the bytes it
/// copies, and `bytes` is not constant for a window id: a banded present copies a fraction of the box
/// (x86 only today, but the counter is arch-neutral), and the per-id censuses are never reset, so a
/// recycled id can mix two different geometries into one window's history. Dividing by the bytes makes
/// a banded present and a whole-box present of the same window directly comparable, which a ratio of
/// raw microseconds is not. `4 KiB` is a scale chosen so the integer division keeps three or four
/// significant figures at every geometry this compositor presents — nothing depends on the unit, only
/// on the RATIO being taken between two numbers in the same one.
///
/// **What the ratio is FOR.** A present that is slow because the copy is slow is slow EVERY time — the
/// same rows, the same bytes, the same core — so its fastest and slowest presents differ by little and
/// `presspread` sits near 1. A present that is slow because the machine underneath it stopped running
/// is slow ONCE, at random, while the window's other presents stay fast, and `presspread` blows out.
/// That is the shape of a QEMU vCPU losing its host timeslice, and it is the only wire-visible way this
/// witness can tell "the copy lost the race with the beam" from "the clock ran while nobody did".
///
/// **This is a CENSUS, not a verdict.** `torn=`, `-> AT-RISK` and their precedence are untouched by
/// WCH-SPREAD: the counter is published beside them and the reading is left to whoever consumes the
/// line. The stall guard that doc's first edition named as the x86 seat's open item now EXISTS —
/// [`H_STALL`]/[`STALL_SPREAD`] in [`stage_note`]'s tear test — and is built on exactly this pair.
static H_MINRATE: [AtomicU64; IDS] = [const { AtomicU64::new(u64::MAX) }; IDS];
static H_MAXRATE: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];
/// WCH-STALL — per-id: slow presents the tear test CONVICTED OF STALLING rather than tearing: the
/// present outran the beam (`present_us > rectscan_us`, [`H_TORN`]'s own test) but its per-4-KiB
/// rate was more than [`STALL_SPREAD`]× this window's established floor. That is the desched shape
/// [`H_MINRATE`]'s doc describes — slow ONCE, at random, beside fast siblings — not the uniformly
/// slow copy a genuine tear risk is. This is the stall guard that doc names as the x86 seat's open
/// item, built on the measurement it asked for.
///
/// The precedence is deliberate and conservative: a window's first present OVERALL can never be
/// convicted (no earned floor exists yet — its own rate seeds it), and a window whose every
/// present is slow keeps counting `torn=` exactly as before (each rate sits near the floor its
/// siblings set) — the guard only diverts outliers against a floor the window itself earned. `stalls=` is
/// printed beside `torn=` so the diverted population stays on the wire; the verdict reads `torn=`
/// alone, so a stall can never manufacture `-> AT-RISK` and metal FORBIDs keep their teeth.
static H_STALL: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
/// WCH-STALL — how many times the window's floor rate a slow present must exceed to be convicted a
/// stall.
///
/// **This constant is arch-conditional, and that is a hardware-population fact rather than a
/// behavioural divergence.** The number is a threshold on a MEASURED distribution, and the two
/// trees measure different machines; pinning one value would either disarm the guard on one of them
/// or make it convict honest presents on the other. Per-tree exact pins, unioned at merge — the
/// WMCTRL ruling. Both pins are cited from their own tree's metal capture below.
///
/// **aarch64 = 8 (the pi seat's boot-8 baseline, unchanged).** There the healthy presspread
/// population is 1–7 sustained, the AT-RISK population is `{32, 33, 84, 136}`, and the 7..32 band is
/// EMPTY — a clean bimodal gap, so 8 is the lowest defensible threshold above the honest population.
/// Each pi outlier is one huge `maxpresent_us` (e.g. 218876 µs) beside a normal floor (2594 µs):
/// the desched shape this guard was built to divert.
///
/// **x86_64 = 256, and the reason is that this machine's honest population has no such gap.**
/// Measured over `rmbp1-boot1/ttyUSB0.log` — 7 boots, 8133 `[wc-h] rollup` lines, every one of them
/// `torn=0 stalls=0 -> TEAR-FREE`, i.e. an entirely HEALTHY population — `presspread=` is a dense
/// continuum, not a pair of clusters:
///
/// ```text
/// 1:4646  2:591  3:511  4:434  6:109  8:143  9:119  10:44  11:100  13:13  14:293  15:394
/// 22:249  27:1  29:17  30:9  33:179  36:1  44:2  45:1  46:78  51:16  53:1  63:1  73:1  74:9
/// 75:23  88:9  91:16  94:14  111:4  112:11  118:94
/// ```
///
/// 20.9% of healthy x86 rollups (1699 of 8133) sit ABOVE 8, the honest ceiling is 118, and the
/// widest interior gap (94..111) is nowhere near clean. The banded-console geometry that produced
/// this tree's hard-won honest `presspread=8` is the same geometry that produces 36, 63, 91 and 112.
///
/// So on x86 the pin cannot be read off a gap; it has to be read off which ERROR is affordable. The
/// guard only ever DIVERTS `torn=` into `stalls=`, so a false conviction SUPPRESSES a tear and
/// disarms the `-> AT-RISK` FORBID, while a false acquittal merely lets a QEMU desched print as a
/// tear — loud, in QEMU, where it is read by a human. At 8, an honest x86 present anywhere in that
/// 20.9% tail that also outran the beam would be silently convicted and its tear suppressed. 256
/// clears the entire observed honest population by 2.2x, and still sits inside the desched band the
/// guard was priced against (58–407, the GR27 discriminator ledger), so it keeps teeth against the
/// worst of the shape it exists for while no MEASURED honest present on this machine can buy a
/// suppression.
///
/// **The x86 population also overlaps that desched band (118 honest vs 58–407 desched), which means
/// `presspread` alone cannot discriminate on this machine at all.** That is precisely why the raise
/// does not ship alone: [`STALL_PRESENT_US`] is the stall-SHAPED replacement assertion, and it
/// separates cleanly on both trees where the ratio does not.
///
/// Integer, because the rates it multiplies are.
#[cfg(target_arch = "aarch64")]
const STALL_SPREAD: u64 = 8;
#[cfg(target_arch = "x86_64")]
const STALL_SPREAD: u64 = 256;

/// WCH-LONGPRES — the STALL-SHAPED assertion: the absolute duration, in microseconds, above which a
/// single present is named a stall in its own right, independent of any ratio.
///
/// **Why this exists.** [`STALL_SPREAD`]'s x86 re-price raises a threshold, and a raised threshold
/// that suppresses verdicts must ship WITH a replacement assertion rather than as a quiet change
/// (the pi seat's caveat, adopted here as a design rule). `presspread` is a RATIO, and a ratio can be
/// blown out from either end — on this x86 tree the tail is driven by an anomalously FAST present at
/// the floor (`minpresent_us` of 23–38 µs beside a normal maximum), which is not a stall at all.
/// A stall has a shape the ratio cannot state: one present that took an ABSURD wall-clock time.
///
/// **The price, from both trees' metal.** Across all 8133 x86 rollups the largest `maxpresent_us`
/// ever recorded is 7276 µs — under half a frame — and the p50 is 2384 µs. The pi's four AT-RISK
/// outliers are single presents of ~218876 µs, thirteen frames. Two frames sits 4.6x above every
/// present either machine has been observed to make while healthy, and 6.5x below the smallest
/// stall-shaped present ever captured. That is the wide two-sided separation `presspread` has on the
/// pi and has lost on x86, recovered in a quantity that does not depend on a floor.
const STALL_PRESENT_US: u64 = 2 * FRAME_US;

/// WCH-LONGPRES — per-id: presents whose wall-clock duration exceeded [`STALL_PRESENT_US`].
///
/// Counted over EVERY present, beside `torn=`/`stalls=` and on the same side of the budget gate, for
/// the reason [`H_TORN`]'s move records: a census that stops at four samples describes a startup
/// burst. Unlike `stalls=`, this counter is NOT a diversion — it does not take a present away from
/// `torn=`, and it does not require the present to have outrun the beam. A present can be both torn
/// and long, and both counters will say so. It therefore cannot weaken any existing verdict; it only
/// adds a reading the ratio cannot express.
static H_LONGPRES: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
/// WCH-LONGPRES — per-id: whether the one-shot `-> STALL` line has already been emitted for this
/// window, so a wedged machine names its first offender and then stops paying UART for the rest.
static H_LONGSAID: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
/// WCH-LONGPRES — per-id: the offending present's measured duration, handed from the recorder to
/// [`stage_flush`] so the naming line is PRINTED outside the clock that timed it.
///
/// This is the same deferral [`emit_sample`] exists for, and for the identical reason: a
/// `serial_println!` inside `stage_note` runs inside WC-G's clock, so the witness would be charged to
/// the very measurement it is reporting. Zero means nothing pending — a present of zero microseconds
/// cannot exceed [`STALL_PRESENT_US`], so the sentinel is unambiguous.
static H_LONGUS: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];

/// WCHFIX — per-id: how many presents actually entered the rate census above. Published as the
/// rollup's `presspop=`, immediately beside `presspread=`, because a ratio without its population
/// is not a reading.
///
/// **The blind spot this closes.** `presspread` is `max/min` over this population. When the
/// population is ONE, `max` and `min` are the same sample and the ratio is `1` BY CONSTRUCTION — not
/// a measurement of evenness, but the arithmetic identity of a single point. A single-present window
/// that also tore therefore printed the exact shape the pi4 spec's single-digit FORBID was written to
/// convict (`presspread=1 -> AT-RISK`), while carrying no evidence whatsoever about which of the two
/// causes produced the tear. The x86 seat measured that as roughly one false red in five runs of an
/// otherwise green suite, concentrated on the shortest and most loaded boots — precisely the desched
/// regime the discriminator exists to EXCUSE.
///
/// **Why a counter rather than a verdict change.** The alternative was to withhold `-> AT-RISK` until
/// two presents exist. That would be a lie about the panel: the window DID tear, the tear counter DID
/// see it, and a witness that reports `TEAR-FREE` over a measured tear is the failure mode WC-K was
/// corrected for. The unmeasurable thing is not the tear, it is the SPREAD — so the population is
/// published beside the spread and the consumer that reads the spread (the spec's FORBID) is the one
/// that declines to convict. The verdict, its precedence and every existing count are untouched.
///
/// **Why not `whole=` or `banded=`.** Those count staged presents, including the zero-byte case the
/// rate recorder skips; using them as the spread's population would assert a point that was never
/// taken. This counts the samples the ratio was actually computed from, so `presspop >= 2` is exactly
/// the condition under which `max` and `min` can be different measurements.
static H_RATEN: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
/// Per-id: composites that did NOT reach the back layer and ran on the direct (pre-WC-H) path — the
/// tearing regime. Excludes the deliberate fixture decline, which is counted separately.
static H_DECLINE: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
/// WCHUN — per-id, per-reason: the same declines [`H_DECLINE`] totals, split by WHICH exit of
/// [`stage_window`](super::wm) produced them. Indexed by the reason constant itself, so slot 0
/// ([`KIND_STAGED`]) is permanently unused and no mapping table has to be kept in step.
///
/// **Why the lumped counter was not enough.** `declines=` is unbudgeted and survives the whole boot,
/// but the only place a decline's REASON was ever written down is the per-sample
/// `[wc-h] win=N staged=no reason=… -> DIRECT` line — and that line spends [`H_TAKEN`], the same
/// four-sample budget the staged presents spend. The budget is therefore gone before the interesting
/// declines arrive, for precisely the reason [`H_BTAKEN`] documents about banded presents: *an
/// instrument whose budget is spent by the control can never see the treatment*. Window creation and
/// first paint stage successfully, burn the budget, and every decline afterwards is counted and
/// anonymous.
///
/// The `pi4-pi1-b1` capture is the case that named this. Its first boot window carries 663
/// `-> UNSTAGED` rollups, all one window, `declines=` climbing 10 → 3281 against `whole=62388`
/// presents — a real, sustained, ~5% fallback into the tearing path — and the boot prints not one
/// `reason=` line for any of them, because that window's four samples went to the three
/// `-> BUFFERED` composites of its first paint. The verdict was reachable and the diagnosis was not.
///
/// The four reasons are four different faults and want different answers: [`DECL_GEOM`] is a
/// degenerate box, [`DECL_CAP`] a box no band can fit (unreachable at any panel this kernel
/// addresses), [`DECL_LOCK`] a core re-entering its own stage entry, [`DECL_ALLOC`] a heap that will
/// not grow. Lumping them makes a permanent cap fallback and a bursty reentrancy read identically.
///
/// Unbudgeted, like [`H_DECLINE`] and for the same reason: the count is what outlives the trace.
static H_DECLBY: [[AtomicU32; DECL_KINDS]; IDS] =
    [const { [const { AtomicU32::new(0) }; DECL_KINDS] }; IDS];
/// One past the largest reason constant [`stage_decline`] can be handed, so [`H_DECLBY`] can be
/// indexed by the constant directly.
const DECL_KINDS: usize = DECL_ROUTE as usize + 1;
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
///
/// **WC-K2r — STILL LIVE. Do not delete this alongside WC-L's erase fixture.** The two are unrelated
/// despite the shared word: WC-L's latch lived in `wm::stage_fill` (the DESKTOP FILL path) and WC-K2
/// retired it, while this one belongs to `wm::stage_window` (the WINDOW path), where the direct
/// fallback still exists and still needs a per-boot witness. `[wc-h] win=N staged=no reason=fixture
/// -> DIRECT` is its line and it is expected on every witness boot.
pub const DECL_FIXTURE: u32 = 5;
/// WC-K2 — not a failure either, and not a staging attempt at all: the erase path no longer
/// publishes, so every vacated box is queued for the compositor's drain BY DESIGN. Only
/// [`erase_defer`] ever carries it; [`stage_window`](super::wm) cannot produce it.
pub const DECL_ROUTE: u32 = 6;

fn decl_name(kind: u32) -> &'static str {
    match kind {
        DECL_GEOM => "geom",
        DECL_CAP => "cap",
        DECL_LOCK => "lock",
        DECL_ALLOC => "alloc",
        DECL_FIXTURE => "fixture",
        DECL_ROUTE => "route",
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
    // The CENSUS is taken first and unconditionally — before the budget is even read — because it is
    // the half that has to survive the budget. Both branches below used to carry their own copy of
    // this pair; hoisting it above the gate is what makes "unbudgeted" a property of the code shape
    // rather than of two call sites agreeing.
    if reason == DECL_FIXTURE {
        H_FIXTURE[i].fetch_add(1, Ordering::Relaxed);
    } else {
        H_DECLINE[i].fetch_add(1, Ordering::Relaxed);
    }
    // WCHUN — and the reason census beside it, on the same terms. Guarded rather than assumed: the
    // reason arrives from another module, and an out-of-range one must lose its breakdown, not panic
    // on the present path. `declines=` still counts it, so the total can never disagree with the
    // verdict; only the split would be short, and `[wc-h]`'s own `?` name for an unknown reason has
    // the same standing.
    if (reason as usize) < DECL_KINDS {
        H_DECLBY[i][reason as usize].fetch_add(1, Ordering::Relaxed);
    }
    let n = H_TAKEN[i].fetch_add(1, Ordering::Relaxed) + 1;
    if n > SAMPLES {
        H_TAKEN[i].store(SAMPLES + 1, Ordering::Relaxed);
        // Past budget the LINE stops but the count must not: an unstaged composite is the thing the
        // verdict is about, and a boot that starts declining after sample 4 has to remain visible.
        return;
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
/// `pub(super)` for the same reason as [`PAYGO_LATTICE_N`]: `video::wm`'s `[wc-d] paygo` census
/// refreshes on this cadence and for these reasons, and a second period written down over there
/// would be a second duty cycle to reason about.
pub(super) const CENSUS_PERIOD_US: u64 = 2_000_000;

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
    // WCH-STALL — classify BEFORE this present folds into the floor below: the comparison must be
    // against the floor the window had EARNED, or a slow first present would be judged against
    // itself. `lo == u64::MAX` (no floor yet) and `bytes == 0` (no rate exists) both decline the
    // conviction and leave the present to `torn=`, which is the conservative direction: the guard
    // may only ever DIVERT a tear count, never invent one, and only when the window itself has
    // proven it can present an order of magnitude faster.
    if present_us > rectscan_us {
        let stalled = bytes != 0 && {
            let rate_ns_4k = present_us.saturating_mul(4_096_000) / bytes as u64;
            let lo = H_MINRATE[i].load(Ordering::Relaxed);
            // `lo != 0` is load-bearing (review finding, 2026-08-18): a present that measures 0 µs
            // folds a floor of ZERO, and `rate > 0 * 8` would then convict EVERY subsequent slow
            // present — torn= suppressed for the window's whole life, the pi4 AT-RISK FORBID
            // permanently disarmed. A zero floor is "no floor", same as MAX: decline and count torn.
            lo != u64::MAX && lo != 0 && rate_ns_4k > lo.saturating_mul(STALL_SPREAD)
        };
        if stalled {
            H_STALL[i].fetch_add(1, Ordering::Relaxed);
        } else {
            H_TORN[i].fetch_add(1, Ordering::Relaxed);
        }
    }
    // WCH-LONGPRES — the stall-SHAPED test, taken beside the ratio test and deliberately INDEPENDENT
    // of it. No `present_us > rectscan_us` precondition: a present that ran for two frames is a stall
    // whether or not the geometry it was copying happens to make that longer than the beam's own scan
    // of the same rows. No floor precondition either — that is the whole point of the quantity. It
    // diverts nothing, so `torn=`, `stalls=` and the verdict precedence are all exactly as they were.
    if present_us > STALL_PRESENT_US {
        H_LONGPRES[i].fetch_add(1, Ordering::Relaxed);
        // Only the FIRST offender is handed to the printer; the counter keeps the rest.
        if H_LONGSAID[i].load(Ordering::Relaxed) == 0 {
            H_LONGUS[i].store(present_us, Ordering::Relaxed);
        }
    }
    H_MAXPRES[i].fetch_max(present_us, Ordering::Relaxed);
    // WCH-SPREAD — the floor and the two rate extremes, taken here for exactly the reasons the tear
    // test and `maxpresent_us` are taken here: they are censuses over EVERY present, and a spread
    // measured over a window's first four is a spread over its startup burst. The cost is one more
    // `fetch_min`, one multiply and one divide on values already in registers — the same price the
    // paragraph above accounts for, and it touches no memory outside three atomics. See [`H_MINRATE`].
    H_MINPRES[i].fetch_min(present_us, Ordering::Relaxed);
    if bytes != 0 {
        let rate_ns_4k = present_us.saturating_mul(4_096_000) / bytes as u64;
        H_MINRATE[i].fetch_min(rate_ns_4k, Ordering::Relaxed);
        H_MAXRATE[i].fetch_max(rate_ns_4k, Ordering::Relaxed);
        // WCHFIX — the population the two extremes were drawn from, incremented in the SAME branch
        // that feeds them so the count can never claim a sample the ratio did not see. One more
        // `fetch_add` on an atomic already in this cache line's neighbourhood. See [`H_RATEN`].
        H_RATEN[i].fetch_add(1, Ordering::Relaxed);
    }
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
    // WCH-LONGPRES — name the first present that exceeded the absolute bound, with the number it
    // measured, once per window. Printed HERE rather than at the record for the reason this
    // function's note gives: `stage_note` runs inside WC-G's clock.
    //
    // The verdict token is on its OWN line, not on the rollup's terminal. `-> TEAR-FREE` /
    // `-> AT-RISK` / `-> UNSTAGED` are matched by two tracks' regression specs (see
    // [`stage_rollup`]'s key-order note), and a fourth terminal arm would change what a line those
    // specs already guard is claiming. A new line is an insertion in the same sense a new field is:
    // it adds an assertion without redefining an existing one. The rollup carries the COUNT
    // (`longpres=`) and the BOUND (`stallbound_us=`); this line carries the evidence.
    let longus = H_LONGUS[i].swap(0, Ordering::Relaxed);
    if longus != 0 && H_LONGSAID[i].swap(1, Ordering::Relaxed) == 0 {
        // `minpresent_us=` beside it is what makes the reading a SHAPE rather than a number: a stall
        // is one absurd present beside a normal floor, which is exactly the pi's 218876-vs-2594. A
        // window whose floor is up there with it is uniformly slow, which is a different fault.
        let minp = H_MINPRES[i].load(Ordering::Relaxed);
        serial_println!(
            "[wc-h] win={} present_us={} bound_us={} minpresent_us={} -> STALL",
            id,
            longus,
            STALL_PRESENT_US,
            if minp == u64::MAX { 0 } else { minp },
        );
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
    // WC-G/M3 — and PAYGO's own census, on the same cadence and from the same site. This is also the
    // only place a deferral may PRINT: `paygo_open` runs inside `begin`, between WC-D's frozen
    // reference and the copy it describes, so it records and this emits. See [`PAYGO_PEND`].
    paygo_flush(id, i);
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
        //
        // FBCON-PACE — and on the ROUTED CONSOLE `span=` no longer describes one printed line. The
        // console coalesces its damage and presents at most once per frame period, so this span is
        // the UNION of every line since its last present: larger and rarer than it used to read, at
        // or near the box height through a boot burst and single-line only when printing is slower
        // than a frame. It is still exactly the rows this present wrote — the definition did not
        // move, the producer did. See `fbcon::route_present_banded`.
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
/// | `pop=all-presents` | `torn` … `presspop` | every present/decline this WINDOW has had |
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
///   run, so a window that tears only under load says so. The pi4 spec FORBIDs it — but reads
///   `presspread=` alongside, because `-> AT-RISK` on its own cannot say WHY the present was slow.
/// - `presspread=` near 1 beside `-> AT-RISK` **and `presspop=` of 2 or more** — the presents are
///   UNIFORMLY expensive, which is what a copy that genuinely cannot keep up with the beam looks like.
///   This is the reading the tear FORBID was written for. `presspop=1` beside the same ratio is NOT
///   that reading and never was: with one sample the max and the min are the same present and the
///   ratio is 1 by construction. See [`H_RATEN`].
/// - `presspread=` in the tens or hundreds beside `-> AT-RISK` — the window has both very fast and
///   very slow presents of the same bytes. A copy does not vary by that factor; a machine that stops
///   running underneath it does. On QEMU without `-icount` that is a host desched being charged to the
///   present phase, and the `torn` count it produced is a measurement of the host, not of the panel.
/// - `-> UNSTAGED` — `declines > 0`; a composite reached the panel through the pre-WC-H direct path.
///   Also FORBIDden.
/// - `banded=0` on the latest line of a window the console is known to be routing damage into —
///   FBCON-DMG is not reaching the compositor at all. **FBCON-PACE qualifies this reading:** a
///   SMALL `banded=` beside a console that is printing steadily is now the expected state, not a
///   defect — the console coalesces per frame period, so `banded=` counts frames, not lines, and
///   watching it collapse while `lines=` keeps climbing is the pacing gate working. The failure
///   this bullet names is `banded=0` *exactly*: not one banded present in the whole window's life,
///   which no amount of coalescing can produce while any routed line is drawn.
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
    // WCH-SPREAD — the present phase's floor and its evenness. `minpresent_us=0` and `presspread=0`
    // both mean the same thing and only that: this window has recorded no present the counter could
    // measure, so neither number exists yet. `H_TORN` is incremented three lines above the rate
    // recorder, so a window that reaches `-> AT-RISK` has recorded a present — the one residual is a
    // present of ZERO BYTES, which the rate recorder skips and which `stage_window`'s degenerate-
    // geometry decline is there to make unreachable. Should one ever arrive, `presspread=0` is in the
    // single-digit class the pi4 spec's FORBID convicts, so the unmeasured case fails SAFE — and
    // `presspop=0` now says so explicitly rather than leaving the reader to infer it from a zero ratio.
    //
    // WCHFIX — `presspop=` is the number of presents the two extremes were taken over, and it is what
    // makes `presspread=` readable at all. At `presspop=1` the max and the min are the SAME sample, so
    // the ratio is 1 by arithmetic and says nothing about evenness; the single-digit reading that the
    // pi4 spec convicts is, on that line, an identity rather than a measurement. The spec keys its
    // FORBID on `presspop` >= 2 for that reason. Nothing here changes the verdict: a window that tore
    // still reads `-> AT-RISK` whatever its population, because the tear was measured and the spread
    // was not.
    //
    // `lo.max(1)` rather than a branch: a present fast enough to round to a rate of zero is a present
    // faster than one nanosecond per 4 KiB, i.e. below the timer's resolution, and a window holding one
    // of those alongside a torn present is as uneven as this counter can report. Saturating it to the
    // largest ratio the numbers allow is the honest reading of that pair, not a special case.
    let minp = H_MINPRES[i].load(Ordering::Relaxed);
    let minpresent = if minp == u64::MAX { 0 } else { minp };
    let lo = H_MINRATE[i].load(Ordering::Relaxed);
    let presspread =
        if lo == u64::MAX { 0 } else { H_MAXRATE[i].load(Ordering::Relaxed) / lo.max(1) };
    // WCHUN — the decline census, split by reason. Printed unconditionally, including the all-zero
    // case: a reader who has to tell "no declines" from "the field is missing on this build" cannot,
    // and a boot whose `declines=` is 0 is exactly the boot whose breakdown proves the counter is
    // wired. Four fixed keys rather than a variable list, for the same reason `minspan_bytes=` is
    // always present beside `minspan=` — the line's shape must not depend on its values.
    //
    // `fixture` is deliberately NOT among them: it has carried its own top-level key since WC-H and
    // is excluded from `declines=`, so repeating it here would file one count under two names.
    // `route` is likewise absent — `stage_decline` cannot be handed it (only `erase_defer` carries
    // `DECL_ROUTE`, and that reports through `[wc-k]`), so a key for it would be a permanent zero
    // asserting nothing.
    let declby = |r: u32| H_DECLBY[i][r as usize].load(Ordering::Relaxed);
    // KEY ORDER IS LOAD-BEARING ACROSS SEATS. `win=`, `scope=`, `declines=` and the terminal
    // `-> {verdict}` are matched in this order by the pi4 track's regression spec, which also relies
    // on `scope=window ` carrying a TRAILING SPACE so its pattern cannot match `scope=window-band`.
    // Everything WC-H2 added — `emit=`, `age_ms=`, the `pop=` markers, `budget=`, `lines=` — and
    // everything WCH-SPREAD added — `minpresent_us=`, `presspread=` — and WCHFIX's `presspop=` is an
    // INSERTION between existing keys. Nothing is renamed, nothing is reordered, and the terminal stays
    // terminal. The new keys go INSIDE the `pop=all-presents` run, beside `maxpresent_us=` whose
    // population they share; putting them after `pop=constant` would have filed two measurements under
    // the marker that means "compile-time constant". WCH-STALL's `stalls=` follows the same insertion
    // rule, directly after `torn=` — the population it was diverted from — and WCHUN's
    // `decl_geom=`/`decl_cap=`/`decl_lock=`/`decl_alloc=` go directly after the `declines=` total they
    // decompose, inside `pop=all-presents` because they share that population exactly.
    //
    // WCH-LONGPRES adds two, by the same rule and on the same reasoning about which marker owns
    // which kind of number: `longpres=` is a MEASURED census, so it goes inside `pop=all-presents`
    // beside the two counters it is read against; `stallbound_us=` is a compile-time constant, so it
    // goes after `pop=constant` beside `frame_us=`, where a reader can see the bound the count was
    // taken against without knowing the source. Both are insertions; the terminal stays terminal.
    //
    // SYNC-FOLD 2026-08-22 — `presspop=` sits DIRECTLY after `presspread=`: the pi4 spec's AT-RISK
    // FORBID and the re-armed x86-witness FORBID both key on that adjacency. Arity is 27 = 27.
    serial_println!(
        "[wc-h] rollup win={} scope={} emit={} age_ms={} pop=budgeted samples={} budget={} pop=all-presents torn={} stalls={} longpres={} declines={} decl_geom={} decl_cap={} decl_lock={} decl_alloc={} fixture={} whole={} banded={} lines={} minspan={} minspan_bytes={} maxpresent_us={} minpresent_us={} presspread={} presspop={} pop=constant frame_us={} stallbound_us={} -> {}",
        id,
        scope,
        emit,
        age_ms,
        taken.min(SAMPLES),
        SAMPLES,
        torn_n,
        H_STALL[i].load(Ordering::Relaxed),
        H_LONGPRES[i].load(Ordering::Relaxed),
        decl_n,
        declby(DECL_GEOM),
        declby(DECL_CAP),
        declby(DECL_LOCK),
        declby(DECL_ALLOC),
        H_FIXTURE[i].load(Ordering::Relaxed),
        H_WHOLE[i].load(Ordering::Relaxed),
        H_BANDED[i].load(Ordering::Relaxed),
        H_LINES[i].load(Ordering::Relaxed),
        minspan,
        minbytes,
        H_MAXPRES[i].load(Ordering::Relaxed),
        minpresent,
        presspread,
        H_RATEN[i].load(Ordering::Relaxed),
        FRAME_US,
        STALL_PRESENT_US,
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

/// WC-K2r — `reason=route` lines get their OWN budget, and a small one.
///
/// The first cut spent [`E_DECL_LINES`] on them, which is the instrument-that-cannot-speak defect in
/// a new costume. Before WC-K2 a deferral was an EXCEPTION and sixteen lines were generous; after it
/// every erase in the system is a deferral, and one metal drag produces up to four per motion report
/// — the budget is gone inside four reports, and everything that shares it goes quiet with it. What
/// goes quiet is not decoration: `erase_drop`'s `-> LOST` line (the `geom`/`cap` fill the panel never
/// received) and the genuine `lock`/`alloc` deferrals that are WC-L's whole subject, i.e. the two
/// classes a boot most needs to be able to report at minute forty.
///
/// Four lines is enough for the spec's REQUIRE to have something to match and for a reader to see
/// the shape of the route on the wire; after that the route is COUNT-ONLY (`defers=` in the rollup),
/// which is the right treatment for a per-motion-report event. The failure classes keep all sixteen.
const E_ROUTE_LINES: u32 = 4;

/// Route deferral lines emitted, capped at [`E_ROUTE_LINES`]. Separate from [`E_DECL_LINES_OUT`] on
/// purpose — sharing the counter is what created the starvation this splits.
static E_ROUTE_LINES_OUT: AtomicU32 = AtomicU32::new(0);

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
/// Fills queued as deferred damage rather than published on the spot.
///
/// **WC-K2r — the WC-L reading of this counter is stale and is replaced, not annotated.** It used to
/// mean "could not reach the back layer on its first attempt", i.e. a contention event. After WC-K2
/// it means "was routed", which is every erase in the boot: `route` deferrals dominate it and
/// `lock`/`alloc` are a garnish. The counter that still carries WC-L's meaning is [`E_REDEFER`] — a
/// box the DRAIN tried and could not stage — and that is the one to read for contention.
///
/// It is also a SNAPSHOT, not a boot total. The `scope=fills` rollup prints once, at sample
/// [`E_SAMPLES`], so `defers=` reports the deferrals seen up to the fourth staged fill and nothing
/// after it. That was already true under WC-L and unremarkable when deferrals were rare; with the
/// route dominating, `defers=4` on a boot that went on to route thousands is the expected reading
/// and not a broken counter. The completeness question belongs to the FORBIDs, per `scope=fills`.
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
/// WC-K2 — desktop fills that reached the FRONT BUFFER from outside a composite publish.
///
/// After WC-K2 `wm::erase` writes no pixel: it queues, and `wm::drain_deferred` — at the head of a
/// composite pass — is the only caller of `wm::stage_fill`. This counts the presents taken with
/// `from_drain == false`, i.e. a desktop fill landing on the glass as its own panel event, ahead of
/// the window repaint that belongs with it. That two-event shape IS the drag seam WC-K2 removed, so
/// it is a structural regression rather than a timing one and it outranks every timing term in the
/// rollup's precedence.
///
/// Zero by construction today. The counter exists because "by construction" is a claim about the
/// caller graph, and a caller graph is exactly the kind of thing a later arc changes without noticing.
static E_OUTSIDE: AtomicU32 = AtomicU32::new(0);

/// WC-K2r — composite passes taken ONLY because the erase queue was non-empty, on the two x86
/// wakeup gates that consume `COMP_PENDING` (`wm::composite`'s re-run loop and its lost-wakeup
/// block). Each one is a desktop fill the pre-fix code would have stranded: it swapped the wakeup to
/// false, asked `any_damaged()` alone, got `no`, and left a queued box with no core committed to it.
///
/// **Reported, never forbidden, and not a completeness claim.** What a reader wants — "no queued box
/// ever outlives the next completed pass" — cannot be answered from inside a boot, and for WC-G's
/// reason restated: the stranded state IS "no further pass arrives", so the detector that would
/// observe it is the pass that does not happen. A residency counter has the same hole (it can only
/// tick when a pass runs). So this counts the fix FIRING instead, which is the falsifiable half: a
/// boot with `rescues>0` is one where the old code dropped a fill, and a boot with `rescues=0` has
/// simply never met the condition. The completeness question stays where WC-K put it, with the
/// FORBIDs.
static E_RESCUE: AtomicU32 = AtomicU32::new(0);

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
    // WC-K2r — the route takes its own small budget; every other reason keeps the full one. See
    // [`E_ROUTE_LINES`] for why sharing them starved the classes that matter.
    if reason == DECL_ROUTE {
        let n = E_ROUTE_LINES_OUT.fetch_add(1, Ordering::Relaxed) + 1;
        if n > E_ROUTE_LINES {
            E_ROUTE_LINES_OUT.store(E_ROUTE_LINES + 1, Ordering::Relaxed);
            return;
        }
    } else {
        let n = E_DECL_LINES_OUT.fetch_add(1, Ordering::Relaxed) + 1;
        if n > E_DECL_LINES {
            E_DECL_LINES_OUT.store(E_DECL_LINES + 1, Ordering::Relaxed);
            return;
        }
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

/// WC-K2r — count a pass rescued by [`E_RESCUE`]'s condition, and name the first one on the wire.
pub fn erase_wakeup_rescue() {
    let n = E_RESCUE.fetch_add(1, Ordering::Relaxed) + 1;
    if n == 1 {
        // One line, at the first occurrence, on the `scope=starve` pattern: the `scope=fills` rollup
        // fires at sample 4 and a rescue by its nature arrives later (it needs a DECLINED pass, which
        // needs two cores compositing at once). A counter whose only home is a rollup that has
        // already printed is a counter nobody reads.
        serial_println!("[wc-k] rollup scope=wakeup rescues={} -> RESCUED", n);
    }
}

/// WC-K2 — record a desktop fill that is about to reach the front buffer from outside a composite
/// publish, and SAY SO on the wire the first time it happens.
///
/// The one-shot line is not decoration and not a duplicate of the rollup field. The `scope=fills`
/// rollup prints ONCE, at sample [`E_SAMPLES`], and cannot retract; a boot whose erase path is
/// healthy for its first four fills and is then handed back a direct publisher by a later arc would
/// otherwise carry a printed `TEAR-FREE` over exactly the defect. Same reasoning that gives
/// `-> STARVED` its own line: a FORBID is only worth having if the boot can still trip it after the
/// rollup has spoken.
pub fn erase_outside_publish(w: usize, h: usize) {
    let n = E_OUTSIDE.fetch_add(1, Ordering::Relaxed) + 1;
    if n == 1 {
        serial_println!(
            "[wc-k] rollup scope=publish box={}x{} outside={} -> UNPUBLISHED",
            w,
            h,
            n
        );
    }
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
    spans: usize,
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
        // `runs=` is the box's row count (the OWED extent, exact on aarch64 where no clip exists);
        // `spans=` is the number of `blit` calls actually issued — on x86 an occlusion-clipped fill
        // fragments rows, so `spans > runs` is the fragmentation tell. WCK4-D2: the two fields keep
        // one meaning on both arches precisely because they are two fields.
        "[wc-k] erase box={}x{} staged=yes rowbytes={} runs={} spans={} contig={} compose_us={} present_us={} rectscan_us={} torn={} -> BUFFERED",
        w,
        h,
        row_bytes,
        h,
        spans,
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
        let outside = E_OUTSIDE.load(Ordering::Relaxed);
        let verdict = if outside > 0 {
            // WC-K2 sits ABOVE `SPLIT`, and the ordering is an argument rather than a preference.
            // Every term below it describes the SHAPE or the TIMING of a present the compositor
            // owns; this one says a desktop fill was published as its own panel event, outside the
            // composite that repaints the windows over it. A well-shaped, untorn present of the
            // wrong publisher is precisely what the drag seam was, and reporting it as `TEAR-FREE`
            // with a footnote is the mistake WC-K made with `-> DIRECT`.
            "UNPUBLISHED"
        } else if nc > 0 {
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
            "[wc-k] rollup scope=fills samples={} rows={} torn={} noncontig={} declines={} outside={} defers={} redefers={} coalesced={} rescues={} maxpresent_us={} frame_us={} -> {}",
            n,
            E_ROWS.load(Ordering::Relaxed),
            torn_n,
            nc,
            decl,
            outside,
            E_DEFER.load(Ordering::Relaxed),
            E_REDEFER.load(Ordering::Relaxed),
            E_COALESCE.load(Ordering::Relaxed),
            E_RESCUE.load(Ordering::Relaxed),
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

/// Whether THIS window still has budget. Gates the present-side checksum so the instrument stops
/// costing anything once it has said what it has to say — per window, not globally.
///
/// It was a global `any()` over all [`IDS`] until GR17 metal convicted it: slots that no window
/// ever occupies (this bench boots three windows of eight) can never spend their budget, so the
/// global test was PERMANENTLY true and this checksum ran on every routed console line for the
/// life of the boot — 3.86 MB of byte-at-a-time FNV per serial print, ~5.6 ms/line measured,
/// ~1.3 s of the witness-armed kepler window and unbounded after it. Per-id, the gate closes the
/// moment this window's four samples are spent, which is what the module's cheapness argument
/// always assumed. A spent id has no reader left to starve: [`begin`] is the only consumer of
/// `APP_CKS`/`APP_SEQ` and it returns `None` for a spent id.
fn budget_left(i: usize) -> bool {
    TAKEN[i].load(Ordering::Relaxed) < SAMPLES
}

/// Record the app-side frame at `SYS_WIN_PRESENT` entry — the checksum of what the owner declared
/// finished, taken while the owner is parked inside the syscall and provably not writing. Called
/// from `wm::present`, after the table lock is dropped and before the composite.
pub fn on_present(id: u32, surf: usize, surf_len: usize) {
    let i = id as usize;
    if i >= IDS {
        return;
    }
    // WCG-CHUNK — a part-paid chunked sample keeps the `app` leg live past the last budget spend:
    // sample 4 spends `TAKEN` at its FIRST chunk, so `budget_left` goes false while the cursor is
    // still mid-box, and without this the leg would go dark for the rest of that sample's chunks.
    #[cfg(feature = "wcg-paygo")]
    let live = budget_left(i) || WCG_CUR[i].load(Ordering::Relaxed) != 0;
    #[cfg(not(feature = "wcg-paygo"))]
    let live = budget_left(i);
    // WC-G/M3 — `paygo_arm` is the deferral gate's other end, and it is not an optimisation: without
    // it this checksum would run on every present for the whole deferral window, because a deferred
    // window's budget stays unspent and `budget_left` therefore stays true. See [`paygo_arm`]. It is
    // `true` and folds away on every build but an x86 `wcg-paygo` one.
    if !live || !paygo_arm(i) {
        return;
    }
    // WCG-CHUNK — full-coverage samples (everything past sample 1) are paid in chunks, so the `app`
    // leg checksums the BAND the next chunk will bracket rather than the whole surface. This is not
    // only symmetry with [`begin`]'s band trio — it is the cost cap that makes chunking viable at
    // all: a chunked sample of the console window spans hundreds of PRESENTS, and a full-surface
    // FNV here would put ~5.7 ms back on every one of them, which is the disease relocated rather
    // than removed. The offset is published beside the hash so `begin` can refuse to compare hashes
    // of different bytes — see [`APP_OFF`].
    #[cfg(feature = "wcg-paygo")]
    if TAKEN[i].load(Ordering::Relaxed) > 0 {
        let cur = WCG_CUR[i].load(Ordering::Relaxed) as usize;
        let lo = cur.min(surf_len);
        let hi = cur.saturating_add(WCG_CHUNK_BYTES).min(surf_len);
        let cks =
            if surf == 0 { FNV_BASIS } else { checksum(surf + lo, hi.saturating_sub(lo)) };
        APP_CKS[i].store(cks, Ordering::Relaxed);
        APP_OFF[i].store(cur as u64, Ordering::Relaxed);
        APP_SEQ[i].fetch_add(1, Ordering::Relaxed);
        return;
    }
    APP_CKS[i].store(checksum(surf, surf_len), Ordering::Relaxed);
    #[cfg(feature = "wcg-paygo")]
    APP_OFF[i].store(u64::MAX, Ordering::Relaxed);
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
    /// WCGSEAM — [`SEAM_WRITES`] as this sample's bracket opened, snapshotted above `t0`. A
    /// convicting [`end`] subtracts it: a non-zero delta is the concurrent writer caught INSIDE the
    /// adjudication bracket.
    seam0: u64,
    t0: u64,
    /// WCG-CHUNK — this probe is one CHUNK of a full-coverage sample: the checksums walked
    /// `band_off..band_off + band_len` of the surface, the read-back resumes at the row that offset
    /// names, and [`end`] banks rather than prints unless the chunk closes the sample or convicts.
    #[cfg(feature = "wcg-paygo")]
    chunk: bool,
    /// WCG-CHUNK — the band's byte offset into the surface (0 and `surf_len` for an unchunked
    /// sample, so every non-chunk path reads the whole surface exactly as before).
    #[cfg(feature = "wcg-paygo")]
    band_off: usize,
    #[cfg(feature = "wcg-paygo")]
    band_len: usize,
}

// ---- WC-G/M3 — the glass read-back, and paying for it as it is used ----------------------------

/// The destination read-back: re-derive what the blit should have landed and compare it against the
/// glass, one probe per SOURCE pixel (the top-left destination pixel of each upscale cell). Returns
/// `(bad, checked)` — the two numbers `fbbad=` prints, with `checked` the honest denominator of
/// whatever coverage this pass actually ran.
///
/// Lifted out of [`end`] with its meaning intact. The bounds are still `draw_window`'s own — the
/// panel clip, the `stride` column bound and the `surf_len` row bound — computed from the geometry
/// the caller passed rather than re-derived, for the reason [`end`]'s note gives: a witness that
/// disagreed with the blit about which pixels exist would report that disagreement as a defect.
///
/// `step` is the probe stride, and it is 1 on every path that existed before M3 — any build, either
/// arch, without the `wcg-paygo` knob — which makes the lattice arithmetic fold away to the
/// original `for col in 0..cols` loop. See [`paygo_open`] for what a `step > 1` pass is and, on
/// the module note, for what it can and cannot catch.
///
/// WCG-CHUNK — `row0..row_cap` is the source-row window THIS invocation may walk and `bounded` arms
/// the time stop; the walk reports how far it actually got (`rows_done`) and the box's whole extent
/// (`rows_total`), so [`end`] can advance the cursor and tell a chunk that closed its band from one
/// that closed the box. Every path that existed before the chunking passes `(0, usize::MAX, false)`
/// — the clamp folds to `rows`, the stop is compiled out or dead, and the walk is the old one byte
/// for byte.
#[allow(clippy::too_many_arguments)]
fn readback(
    fb: &FrameBuffer,
    surf: usize,
    surf_len: usize,
    pw: usize,
    ph: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    stride: usize,
    scale: usize,
    step: usize,
    row0: usize,
    row_cap: usize,
    bounded: bool,
    occ_before: &super::wm::OccSnap,
    occ_after: &super::wm::OccSnap,
) -> (usize, usize, usize, usize, usize, usize) {
    #[cfg(not(feature = "wcg-paygo"))]
    let _ = bounded;
    let mut checked = 0usize;
    let mut bad = 0usize;
    // GR21/WCD-OCC — probes that mismatch AND lie under a higher window. Charged to neither `bad`
    // nor the verdict: a higher window legitimately owns the destination probe. PARITY §6.2 — counted
    // on both arches now that `wm::occ_clip` withholds those probes' pixels on both.
    //
    // ARCH-PARITY (rmbp-7) — that sentence was written before the code did it. The ATTRIBUTION
    // below stayed `target_arch = "x86_64"`-gated, so aarch64 charged every occluded mismatch to
    // `bad` and this counter never left 0 there. Fixed at the gate; see the note there.
    let mut occluded = 0usize;
    if x >= pw || y >= ph || scale == 0 || stride < 4 || step == 0 {
        return (bad, checked, step, occluded, row0, 0);
    }
    let cols = (pw - x).div_ceil(scale).min(w).min(stride / 4);
    let rows = (ph - y).div_ceil(scale).min(h).min(surf_len / stride);
    // PAYGO — the lattice COLLAPSES on a rect narrower than its own step, and it has to.
    //
    // The claim the sampled pass rests on is "every row is probed", which is what makes a full-row
    // band un-missable. That claim is false when `cols < step`: the per-row phase runs `row % step`,
    // so every row whose phase lands at or past `cols` probes ZERO pixels, and a narrow window would
    // be sampled on one row in `step` while its line still said `coverage=lattice16`. Not
    // hypothetical — a 4x-upscaled 8-px-wide content rect gives `cols == 8`, and the reviewed boot
    // has `win=3` running `fbbad=0/64` at `scale=8x`.
    //
    // Below the step there is no coverage to buy, so the honest answer is to take the pass at FULL
    // coverage and say so. The effective step is RETURNED, not just used, so the `coverage=` marker
    // is derived from what the walk actually did rather than from what was asked for — the same rule
    // that put the step in a static rather than recomputing it in `end`.
    let step = if cols < step { 1 } else { step };
    // WCG-CHUNK — the walk's window, and its progress report. `r1` clamps the ask to the box;
    // `rows_done` starts at the plan and is pulled back by the time stop. With `row0 = 0` and an
    // uncapped `row_cap` this is `0..rows`, the loop this always was.
    let r1 = rows.min(row_cap);
    // `mut` is dead where the time stop folds away (any build without the knob, either arch) —
    // allowed rather than cfg-forked, so the binding stays one line on every build.
    #[allow(unused_mut)]
    let mut rows_done = r1.max(row0);
    #[cfg(feature = "wcg-paygo")]
    let t_walk0 = now_cycles();
    for row in row0..r1 {
        // WCG-CHUNK — the time stop. Whole rows only (checked between rows, never mid-row, so an
        // upscale cell is always probed whole), and at least one row per chunk (`row > row0`) so a
        // chunk always makes progress and the cursor cannot wedge. WALL time, deliberately: on
        // metal a Kepler-BAR row of the console costs ~0.8 ms and a chunk stops after two or three
        // rows; under QEMU the probes are RAM reads and [`WCG_CHUNK_BYTES`]' row cap is what sets
        // the chunk size instead — either way no composite holds the gate past ~[`WCG_CHUNK_US`]
        // plus one row's overshoot.
        #[cfg(feature = "wcg-paygo")]
        if bounded
            && row > row0
            && cycles_to_us(now_cycles().saturating_sub(t_walk0)) >= WCG_CHUNK_US
        {
            rows_done = row;
            break;
        }
        let row_base = row * stride;
        let dy = y + row * scale;
        // PAYGO — the lattice's per-row phase. A full pass (`step == 1`) starts at column 0, which is
        // the pre-M3 loop exactly. A sampled pass rotates its first column by one per row, so over
        // any `step` consecutive rows every column is probed exactly once and a one-pixel-wide
        // vertical defect cannot sit in the gaps for longer than that. EVERY row is visited either
        // way, which is what makes a full-row band un-missable at any step.
        let first = if step == 1 { 0 } else { row % step };
        // WC-G/M3 — one word window per destination row. New on x86; the other arches keep the
        // per-probe `read_pixel` call this loop has always made. See [`GlassRow`].
        #[cfg(target_arch = "x86_64")]
        let mut glass = GlassRow::new(fb, dy);
        let mut col = first;
        while col < cols {
            // SAFETY: identical bound to `draw_window`'s read —
            // `row < surf_len / stride` and `col < stride / 4`.
            let want = unsafe {
                core::ptr::read_unaligned((surf as *const u8).add(row_base + col * 4) as *const u32)
            } & 0x00FF_FFFF;
            #[cfg(target_arch = "x86_64")]
            let got = glass.pixel(x + col * scale);
            #[cfg(not(target_arch = "x86_64"))]
            let got = fb.read_pixel(x + col * scale, dy);
            if let Some(got) = got {
                checked += 1;
                if got != want {
                    // GR21/WCD-OCC — attribute to an occluder before charging `fbbad`. A probe
                    // covered by the pre-blit OR the read-back occluder set is a pixel a higher
                    // window owns, exactly the `[wc-d]` treatment. Reached only on a probe that
                    // already disagrees, so a clean pass pays nothing.
                    //
                    // ARCH-PARITY (rmbp-7) — the attribution was `target_arch = "x86_64"`-gated
                    // while the aarch64 arm charged every mismatch straight to `bad`, and the
                    // comment above `checked` and [`OccNote`]'s own doc BOTH already said the
                    // count was taken on both arches. The claim was false and the gate was the
                    // reason: `wm::OccSnap`, `occluders_above`, `occ_excuse` and the call sites
                    // that hand these two snapshots to [`begin`]/[`end`] are gated
                    // `feature = "witness"` with no arch term, so an aarch64 witness build
                    // populates real occluder boxes and then threw them away — charging `fbbad`
                    // for pixels a higher window legitimately owns while `occluded=` printed 0.
                    // A false denominator, and at scale a manufactured `-> FAIL`. There is no
                    // arch content here to gate: `covers` is integer box arithmetic.
                    if occ_before.covers(x + col * scale, dy)
                        || occ_after.covers(x + col * scale, dy)
                    {
                        occluded += 1;
                    } else {
                        bad += 1;
                    }
                }
            }
            col += step;
        }
    }
    (bad, checked, step, occluded, rows_done, rows)
}

/// WC-G/M3 (x86 only) — a one-word window over the WC-mapped glass, so a destination row's probes
/// cost aligned 64-bit reads of the PCIe aperture instead of three byte reads apiece.
///
/// ### Why the read shape had to change
///
/// [`super::FrameBuffer::read_pixel`] takes THREE volatile byte reads per pixel. That is the right
/// primitive for a format-aware one-off, and on this platform it is the wrong one for a full-surface
/// sweep: the Kepler framebuffer is write-combining PCIe memory, so each of those reads is an
/// uncached round trip to the device with nothing in the path to prefetch, cache or combine them.
/// M1's `readback_us=` measured the consequence directly — the read-back is the phase that dominates
/// an armed pass. The probed pixels are 4 bytes wide and 4-aligned, so an 8-aligned 64-bit read
/// covers one or two of them: a contiguous probe run collapses to one transaction per two pixels and
/// a strided one to one per pixel, against three per pixel before. The x86 target this kernel builds
/// (`x86_64-unaos.json`) carries `+sse,+sse2`, but a scalar `u64` is the widest load that is
/// unconditionally sound here — nothing in this path establishes that the SSE state is live for
/// kernel code — so 64 bits is the width, deliberately.
///
/// ### Why it is the same measurement
///
/// [`Self::pixel`] reproduces `read_pixel`'s tests exactly and in the same order: off-panel, past the
/// mapped length, and a layout with no colour inverse each yield `None` and so are not counted, and
/// the Rgb/Bgr decode reads the same three bytes in the same order. Wherever a precondition of the
/// wide path fails — a pixel that is not 4 bytes, a base that is not 8-aligned, or a probe whose
/// containing word would run past the mapped length — it DELEGATES to `read_pixel`, so the fallback
/// is the original code rather than a second implementation of it that could drift from it.
///
/// The cached word is only ever reused by a probe lying inside the SAME 8 bytes, and that probe's
/// value came out of the same transaction. Two adjacent pixels from one consistent snapshot is not a
/// stale read of one — it is strictly better than two byte-triples taken microseconds apart, which is
/// what the old loop did.
#[cfg(target_arch = "x86_64")]
struct GlassRow<'a> {
    fb: &'a FrameBuffer,
    y: usize,
    /// Whether the wide path may run at all. False makes every probe a `read_pixel` delegation, so
    /// an unusual panel degrades to the original cost rather than to a wrong answer.
    wide: bool,
    bgr: bool,
    base: usize,
    len: usize,
    /// Byte offset of this row's pixel 0 from `base`.
    row_off: usize,
    /// The last word read, and its offset from `base`. `usize::MAX` means nothing is cached, and no
    /// valid offset can collide with it: a cached word always satisfies `off + 8 <= len`.
    word: u64,
    word_off: usize,
}

#[cfg(target_arch = "x86_64")]
impl<'a> GlassRow<'a> {
    #[inline]
    fn new(fb: &'a FrameBuffer, y: usize) -> Self {
        let info = fb.info();
        let base = fb.base();
        let wide = base != 0
            && base % 8 == 0
            && info.bytes_per_pixel == 4
            && matches!(info.pixel_format, PixelFormat::Rgb | PixelFormat::Bgr)
            && y < info.height;
        Self {
            fb,
            y,
            wide,
            bgr: info.pixel_format == PixelFormat::Bgr,
            base,
            len: fb.len(),
            row_off: y.wrapping_mul(info.stride).wrapping_mul(4),
            word: 0,
            word_off: usize::MAX,
        }
    }

    /// One destination pixel as `0x00RRGGBB` — [`super::FrameBuffer::read_pixel`]'s exact contract,
    /// including which coordinates yield `None` and therefore go uncounted.
    #[inline]
    fn pixel(&mut self, x: usize) -> Option<u32> {
        if !self.wide || x >= self.fb.width() {
            return self.fb.read_pixel(x, self.y);
        }
        let off = self.row_off + x * 4;
        // `read_pixel`'s length test, character for character: it reads three bytes, so it accepts
        // exactly `off + 3 <= len`.
        if off + 3 > self.len {
            return None;
        }
        let woff = off & !7usize;
        if woff + 8 > self.len {
            // The mapping ends inside this pixel's word. Three byte reads still fit where a 64-bit
            // one does not, so this probe takes the original path and is counted just the same.
            return self.fb.read_pixel(x, self.y);
        }
        if self.word_off != woff {
            // SAFETY: `base` is 8-aligned and `woff` is a multiple of 8, so `base + woff` is
            // 8-aligned; `woff + 8 <= len` keeps the read inside the mapped aperture. Volatile for
            // the same reason `read_pixel`'s byte reads are — this has to be a real read of the
            // scan-out, never folded with the stores the blit has just performed.
            self.word = unsafe { core::ptr::read_volatile((self.base + woff) as *const u64) };
            self.word_off = woff;
        }
        // `off - woff` is 0 or 4: a 4-aligned 4-byte pixel never straddles an 8-aligned boundary.
        let v = (self.word >> (8 * (off - woff))) as u32;
        let (a, b, c) = (v & 0xFF, (v >> 8) & 0xFF, (v >> 16) & 0xFF);
        Some(if self.bgr { (c << 16) | (b << 8) | a } else { (a << 16) | (b << 8) | c })
    }
}

// ---- WC-G/M3 — PAYGO: the battery stops being something the boot pays for all at once ----------

/// PAYGO — the lattice's column step: a sampled pass probes every `PAYGO_LATTICE_N`th source pixel
/// of every row instead of all of them.
///
/// Sixteen, and the figure is a coverage decision before it is a cost one. It is what the pass can
/// still CATCH that fixes it: a horizontal garble run of `PAYGO_LATTICE_N` pixels or more contains a
/// probe in every row it touches, whatever the row's phase, so the band-shaped smearing WC-G was
/// built for is caught deterministically rather than probabilistically. Sixteen destination pixels
/// is 64 bytes at this panel's layout — the width of a cache line, and far narrower than any defect
/// this witness has ever convicted. The cost follows from the coverage and not the other way round:
/// one probe in sixteen is ~1/16 of a full pass's probes.
///
/// `pub(super)` because `video::wm`'s `[wc-d]` read-back samples on the SAME lattice under the SAME
/// knob, and a second `16` written down over there is a second figure to keep in step by hand. See
/// [`paygo_clock`] for the same argument about the threshold and the clock.
#[cfg(feature = "wcg-paygo")]
pub(super) const PAYGO_LATTICE_N: usize = 16;

/// The `coverage=` marker has to NAME the step, and it is a `&'static str` — [`coverage_note`]
/// explains why — so the literal and the constant are pinned together at compile time rather than
/// trusted to be kept in step by hand. A `coverage=lattice16` line over a step of 8 would be the
/// exact class of instrument this module keeps convicting: one whose wire says something its code
/// does not do.
#[cfg(feature = "wcg-paygo")]
const _: () = assert!(
    PAYGO_LATTICE_N == 16,
    "the `coverage=lattice16` literal must track PAYGO_LATTICE_N"
);

/// PAYGO — time SINCE KERNEL ENTRY past which a window's DEFERRED, full-coverage samples may open.
///
/// The threshold is a wall-clock reading and not a phase marker, deliberately: nothing observable
/// inside this module can tell "the boot burst is over" from "the next app has not launched yet",
/// which is [`W_SAMPLES`]'s standing law applied to a new question. An elapsed time is at least a
/// fact about the boot rather than an inference about it, and its failure mode is benign in the one
/// direction that matters — a threshold set too late costs coverage that the wire then reports as
/// unspent budget, where a phase predicate that fired early would silently put the cost back on the
/// boot it was meant to leave.
///
/// Fifteen seconds sits past the armed x86 boot this arc reshapes and well inside any session a
/// desktop is actually used in. A window still compositing then pays its remaining three samples at
/// full coverage; a window that has stopped keeps them unspent, which is exactly what this module
/// already does with a window that stops presenting.
#[cfg(feature = "wcg-paygo")]
pub(super) const PAYGO_DEFER_MS: u64 = 15_000;

/// PAYGO — milliseconds since KERNEL ENTRY, or `None` while that question has no answer yet.
///
/// ### Why this is not `cycles_to_us(now_cycles())`, which is what it used to be
///
/// Two independent defects lived in that one expression, and the second only bites on metal.
///
/// **It measured from RESET, not from entry.** On x86 `now_cycles()` is `rdtsc`, which counts from
/// processor reset — so the raw value includes firmware. On the 2012 rMBP bench, Apple EFI POST can
/// plausibly eat most or all of fifteen seconds before the kernel is even entered, which means the
/// gate could be open at the first composite and the deferral would silently never engage. The wire
/// would still have said `-> PAID` with `deferred=0`, which is the worst available outcome: a
/// feature reporting success for work it never did. This is not a new hazard and it has a settled
/// answer in this tree — [`crate::clock::logts_now`] subtracts [`crate::bootpace::origin_cycles`]
/// for exactly this reason, and so does every BPACE/GPACE `t=`. This does the same.
///
/// **It scaled before it divided.** `cycles_to_us` multiplies by 1e6 first, which is correct for the
/// small DELTAS it was written for and overflows a `u64` at roughly 1.9 h of an absolute counter —
/// after which the reading wraps and the gate's answer becomes noise. Scaling to milliseconds
/// instead of microseconds moves that horizon out to ~76 days, and `saturating_mul` makes even that
/// degrade toward a large reading (gate open) rather than wrap to a small one (gate stuck shut).
///
/// ### Why an unknown rate DEFERS rather than guesses
///
/// [`crate::bootpace::origin_hz`] is 0 until `apic::calibrate` has run. The old code reached
/// `cycles_to_us`, whose uncalibrated fallback is 1.25 GHz against a real ~2.693 GHz part — a rate
/// low by a factor of ~2.15, so a 15 s threshold would have opened at ~7 s of real time and put the
/// cost back on the boot this arc exists to take it off. Returning `None` and DEFERRING is the
/// conservative direction: an unknown clock delays coverage, which the wire reports as unspent
/// budget, where a guessed clock silently spends it early. The same reasoning `origin_hz`'s own note
/// gives — 0 means print raw ticks, never fabricate a millisecond.
#[cfg(feature = "wcg-paygo")]
fn since_entry_ms() -> Option<u64> {
    let origin = crate::bootpace::origin_cycles();
    let hz = crate::bootpace::origin_hz();
    if origin == 0 || hz == 0 {
        // No `entry` stamp, or no calibrated rate: "since entry" does not exist to measure yet.
        return None;
    }
    Some(now_cycles().wrapping_sub(origin).saturating_mul(1000) / hz)
}

/// PAYGO — the deferral clock and its verdict, read ONCE. The one definition both witnesses gate on.
///
/// `video::wm`'s `[wc-d]` read-back defers on exactly this policy — same knob, same threshold, same
/// entry stamp — so it reads the answer from here rather than keeping a second copy of the
/// arithmetic. Two gates that agree today and disagree after one of them is edited is the drift this
/// module keeps convicting, and [`since_entry_ms`]'s ledger is a long list of ways the expression
/// can be got wrong: measured from reset instead of entry, scaled before it divided, guessed off an
/// uncalibrated rate. There is no version of "wc-d writes its own" that does not eventually
/// re-acquire one of those.
///
/// Returns `(since_entry_ms, clock, payable)`. `clock` is the `clock=` field's value on the wire and
/// disambiguates a real zero from an absent one: `("unarmed", false)` says the entry stamp or the
/// TSC calibration was not there to measure against, which is the state the gate DEFERS in, where
/// `since_entry_ms=0 clock=entry` would be a genuine reading taken at entry.
///
/// [`paygo_arm`] and [`paygo_note`] both go through it too, deliberately — an exported helper that
/// the exporting module does not itself use is a second implementation waiting to happen.
#[cfg(feature = "wcg-paygo")]
pub(super) fn paygo_clock() -> (u64, &'static str, bool) {
    match since_entry_ms() {
        Some(ms) => (ms, "entry", ms >= PAYGO_DEFER_MS),
        None => (0, "unarmed", false),
    }
}

/// PAYGO — per-id: the probe step granted to the sample currently open, so [`end`] walks the glass
/// on the same terms [`begin`] admitted the sample on and the `coverage=` marker is read from the
/// same cell as the step it describes. A marker derived independently of the walk could disagree
/// with it, and a `coverage=` that misreports its own pass is worse than no marker at all.
#[cfg(feature = "wcg-paygo")]
static PAYGO_STEP: [AtomicU32; IDS] = [const { AtomicU32::new(1) }; IDS];

/// PAYGO — per-id: blits the deferral gate declined to sample. UNBUDGETED and taken before any
/// other test, on WC-H2's rule: past a gate the LINE stops and the count must not, because a
/// counter that stops counting is an instrument that lies. This one is what makes "the battery is
/// waiting" a quantity rather than an impression.
#[cfg(feature = "wcg-paygo")]
static PAYGO_DEFERRED: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];

/// PAYGO — per-id: whether this window has ever been declined, i.e. whether it is in the deferring
/// regime at all. NOT a print latch any more, and the difference is [`paygo_refresh`]'s whole reason
/// for existing — see [`PAYGO_EMIT`].
#[cfg(feature = "wcg-paygo")]
static PAYGO_SAID: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];

/// PAYGO — per-id: a `state=waiting` line owed but not yet printed.
///
/// **The print does not happen where the decline happens, and that is a correctness fix rather than
/// a tidiness one.** [`paygo_open`] runs inside [`begin`], which `video::wm`'s composite pass calls
/// BETWEEN `verify_reference`'s snapshot of the content rect and `draw_window`'s copy of it. With the
/// console routed into a window, a `serial_println!` from there lands in that window's OWN surface —
/// so the bytes `draw_window` then copies are not the bytes WC-D froze, and WC-D reads its own
/// witness's output as corruption. The panel-side half is muted by `fbcon`'s `PANEL_MUTE_TAGS`, but
/// a `bootlog` build compiles that mute out, and `arroyo`'s `x86-all` leg compiles `witness`,
/// `wcg-paygo`, `wc` and `bootlog` together — so the configuration is reachable, not hypothetical.
/// Nor do the two instruments' one-shots protect each other: `verify_reference` returns `None` for a
/// row that is not `presented` or whose band is empty, and [`begin`] has no `presented` test at all,
/// so the passes they claim can and do desynchronize.
///
/// So the decline is RECORDED here and printed from [`stage_flush`] — the site this module already
/// established for exactly this hazard, after `wcg::end` has stopped the clock and after the copy
/// that WC-D's frozen reference describes. One pending slot per id, like [`H_PEND`], and lost
/// overlaps cost a trace line and never a census: [`PAYGO_DEFERRED`] is incremented at the decline.
#[cfg(feature = "wcg-paygo")]
static PAYGO_PEND: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];

/// PAYGO — per-id: `[wc-g] paygo` lines emitted for this window so far. Printed as `emit=`, one-based.
///
/// **The line it ordinals used to be a one-shot, and a one-shot here was WC-H2's convicted shape
/// wearing new clothes.** The first cut printed `deferred=` exactly once, at the first decline —
/// where it is 1 BY CONSTRUCTION, so the field restated its own trigger and carried no information
/// at all. The census went on climbing behind it, and the only other reading was at completion. A
/// window that never completes its budget — which under deferral is every window the operator does
/// not keep compositing for fifteen seconds — therefore reported a thousand declines as `deferred=1`
/// and never printed a rollup either. That is precisely the defect [`H_MINSPAN`] and [`H_TORN`] were
/// convicted of: a line that fires when something LATCHES can only ever describe the moment it
/// latched.
///
/// The fix is the one this module already built rather than a second mechanism: the waiting line is
/// re-emitted on [`CENSUS_PERIOD_US`] cadence from [`stage_flush`], gated on the census having
/// actually moved, so `deferred=` is a RUNNING census with the moment it was taken stamped on it
/// (`since_entry_ms=`) and its ordinal among this window's paygo lines (`emit=`). The reader's rule
/// is the module's standing one: for any `win=`, the greatest `emit=` supersedes every earlier line,
/// and these lines are never summed — they are snapshots of a monotone total, not deltas.
#[cfg(feature = "wcg-paygo")]
static PAYGO_EMIT: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];

/// PAYGO — per-id: `now_cycles()` as of the END of the most recent paygo emission. The refresh's rate
/// gate and its mutual exclusion both, exactly as [`H_LASTROLL`] serves the rollup: two cores
/// flushing the same window at once both observe this value, and only the one whose
/// `compare_exchange` succeeds prints.
#[cfg(feature = "wcg-paygo")]
static PAYGO_LASTROLL: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];

/// PAYGO — per-id: [`PAYGO_DEFERRED`] as of the most recent emission. The delta gate: a window whose
/// declines have not moved has nothing new to say, and reprinting an unchanged line would spend
/// serial time to restate the previous one. A window that stops compositing therefore goes quiet with
/// its last line describing its last active state — the same steady-state behaviour
/// [`census_refresh`] has.
#[cfg(feature = "wcg-paygo")]
static PAYGO_LASTCENSUS: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];

/// PAYGO-TERM — per-id: this TENANT's battery has spoken its closing line, so the wire is shut.
///
/// **What "terminal" has to mean.** A terminal that fires and then keeps talking is the shape this
/// module keeps convicting, and the first cut of pay-at-close had it: [`paygo_flush`]'s census
/// re-emission is gated on `PAYGO_SAID != 0 && TAKEN < SAMPLES && the census moved`, and a
/// `state=closed … -> UNSPENT` moves none of those. So the wire read `closed emit=6` and then
/// `waiting emit=7` behind it — and under this module's own reader rule (for any `win=`, the greatest
/// `emit=` supersedes) the terminal was superseded by the line that came after it and effectively
/// disappeared. This cell is the state that makes the rule true: set BEFORE the terminal is printed,
/// so the terminal's `emit=` is the greatest this tenant will ever carry, and read by
/// [`paygo_flush`] so nothing is printed after it.
///
/// **Per TENANT, not per slot, and it is the only paygo cell that is.** `TAKEN` and `PAYGO_SAID`
/// survive a recycle by design (that is this module's standing behaviour and not this arc's to
/// change), and `PAYGO_EMIT`/`PAYGO_DEFERRED` must survive it or `emit=` stops being monotone per id
/// and the reader's supersession rule breaks across the recycle. But "has this battery said its last
/// word" is a fact about the WINDOW, and a slot's second tenant is a different window: leaving this
/// set would deny every later tenant of a recycling slot its own terminal — the silent close the
/// terminal exists to remove, reintroduced for 6 of the 7 windows slot 3 hosts in the s73 capture.
/// [`paygo_recycle`] clears it, from `wm::create_inner`, beside the wc-d latches that re-arm there
/// for the same reason.
#[cfg(feature = "wcg-paygo")]
static PAYGO_CLOSED: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];

// ---- WCG-CHUNK — the full-coverage glass read-back, paid in time-bounded chunks -----------------

/// WCG-CHUNK — the wc-g half of the GR27 launch-stall law: no composite pass may hold the gate —
/// IRQs masked, on the presenting app's own core — for an unbounded glass read-back.
///
/// Boot A/Ab (metal, 2026-08-12) measured this instrument's own full-coverage passes on the console
/// window at `readback_us=488744..732797` PER witnessed present (`wit_us=1616524` over the battery)
/// — the same disease the wc-d stage-2 verify had at ~1.26 s, one instrument over, and the launch
/// arc's commit (24ac6b79) named it for this one. The cost hides in the boot burst today only
/// because the fixtures pre-pay the slots; any live full sample lands it on a present.
///
/// So a FULL-coverage sample (samples 2..[`SAMPLES`], the ones the deferral gate matures) is now
/// paid in CHUNKS: each admitted blit brackets a BAND of the surface — the checksums walk the band's
/// bytes, the read-back walks the band's rows from a per-id resume cursor for at most this many
/// microseconds (whole source rows, at least one) — banks the clean counts, and hands the budget
/// back; the sample line prints ONCE, cumulative, when the cursor closes the box, so the wire keeps
/// one line per sample and every gate pattern its shape. An exceptional chunk (COHER, RACE, BLIT)
/// prints immediately, chunk-local, ` band=` naming the rows it walked, and closes the sample the
/// way one bad verdict always has. Sample 1 keeps its LATTICE single pass — the small term (~100 ms
/// on the console, against ~600+ ms per full pass) and the shape x86-witness.spec REQUIREs first
/// (`seq=0 … coverage=lattice16`) — exactly as wc-d's stage 1 kept its band-clipped single pass.
/// A knob-off build, on either arch, is the old walk byte for byte.
///
/// Unlike wc-d, NO band-containment decline is needed: wc-d's reference is frozen pre-blit for the
/// band the present offered, but wc-g re-derives `want` from the LIVE surface at read-back time, and
/// the compositor's contract is that the glass agrees with the surface after every composite — so
/// any admitted composite can carry a chunk over any rows, and progress rides every present.
#[cfg(feature = "wcg-paygo")]
const WCG_CHUNK_US: u64 = 2_000;

/// WCG-CHUNK — the per-chunk CHECKSUM band, in bytes. Two jobs, mirroring `wm::WCD_CHUNK_ROWS_MAX`:
///
/// * it bounds what the `blit`/`civac`/`after` legs (and [`on_present`]'s `app` leg) walk per chunk.
///   The trio is cacheable RAM at ~1.5 ns/byte, so 32 KiB is ~50 us per leg — noise against the
///   glass walk the time stop governs — where re-running the FULL-surface trio per chunk would have
///   put ~17 ms of RAM reads on every chunk of the console window and multiplied the total checksum
///   work by the chunk count. The legs adjudicate the SAME questions band-scoped: `blit != civac`
///   is an alias-attribute mismatch in the band, `blit != after` is the owner writing the band
///   mid-copy — which is exactly the race that would invalidate this chunk's `want` re-derivation —
///   and across the battery the cursor covers every byte of the surface. What narrows is only the
///   incidental wider net (a mid-copy write to rows this chunk is NOT verifying), and those rows
///   get their own bracket when their chunk arrives.
/// * it caps the read-back rows a chunk may claim on hosts where probes are RAM-fast and the time
///   stop never fires (QEMU): `32 KiB / stride` rows — six rows of the 1312-px console, a whole box
///   for anything smaller — each still a bounded hold.
///
/// The widest panel this kernel drives is 8192 px = 32 KiB of stride, so the band always covers at
/// least one whole source row and the "whole rows, at least one" law needs no second mechanism.
#[cfg(feature = "wcg-paygo")]
const WCG_CHUNK_BYTES: usize = 32 * 1024;

/// WCG-CHUNK — per-id: the SURFACE BYTE OFFSET the running full sample's next chunk resumes at.
/// Always a whole-row multiple of the stride the advancing chunk saw; `0` doubles as "no chunked
/// sample in flight" (the first chunk of a sample is the one that starts at 0, and it resets the
/// banked sums). Advanced only by a clean banked chunk; reset by every sample close and by
/// [`paygo_recycle`].
#[cfg(feature = "wcg-paygo")]
static WCG_CUR: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];

/// WCG-CHUNK — per-id: a chunk is between [`begin`] and [`end`] RIGHT NOW. The single-walker latch:
/// `wcd_admit`'s CAS serializes wc-d's chunks, but `begin` has no state machine and
/// `paygo_service`'s own note records that two lanes can composite one window at once — two chunks
/// walking the same cursor would double-bank `checked` and print a denominator the box does not
/// have. Claimed before the budget spend, released on every exit of `end`.
#[cfg(feature = "wcg-paygo")]
static WCG_BUSY: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];

/// WCG-CHUNK — per-id telemetry for the paygo `-> PAID` terminal: `chunks=` and `hold_max_us=`,
/// AFTER the verdict token (the suffix position `wm::wcd_paygo_note`'s B1 review fixed — the bench
/// serial-analyzer's `PAYGO_RE` matches `clock=(\w+) taken=(\d+)` contiguously with no `$` anchor,
/// and the gate's PAID REQUIRE ends at `-> PAID`, so a suffix is invisible to both consumers).
///
/// THE FALSIFIER: Boot A/Ab measured the unchunked read-back at 488 744–732 797 us per pass, so the
/// next metal boot's `hold_max_us` must sit two orders below that (a few thousand us:
/// [`WCG_CHUNK_US`] plus one source row's overshoot plus ~150 us of band checksums) or the fix did
/// not land.
///
/// SPAN, stated: `hold_max_us` is the max over chunks of `cks_blit_us + civac_us + cks_after_us +
/// readback_us` — the witness's own four phases, the same sum `wit_us=` accumulates. It excludes
/// the copy itself (`us=`, work the composite does witness or no witness) and [`on_present`]'s
/// band checksum (bounded by [`WCG_CHUNK_BYTES`], paid at present entry outside the composite).
/// Reset at recycle only — the pair describes the tenant's whole battery, all chunked samples.
#[cfg(feature = "wcg-paygo")]
static WCG_CHUNKS: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];
#[cfg(feature = "wcg-paygo")]
static WCG_HOLD_MAX_US: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];

/// WCG-CHUNK — per-id banked sums for the cumulative closing line: silent clean chunks add their
/// `checked`/`occluded` here (their `bad` is zero — that is what kept them silent), the `prof`
/// phases and bytes sum beside them, and `USMAX` carries the worst single copy (`us=`) so the
/// closing line's `slow=` is drawn from the sample's worst blit rather than its last. Single-writer
/// by construction ([`WCG_BUSY`]), so `Relaxed` throughout; reset by the first chunk of each sample
/// (cursor at 0).
#[cfg(feature = "wcg-paygo")]
static WCG_ACC_CHECKED: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];
#[cfg(feature = "wcg-paygo")]
static WCG_ACC_OCC: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];
#[cfg(feature = "wcg-paygo")]
static WCG_ACC_USMAX: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];
#[cfg(feature = "wcg-paygo")]
static WCG_ACC_BYTES: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];
#[cfg(feature = "wcg-paygo")]
static WCG_ACC_BLITUS: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];
#[cfg(feature = "wcg-paygo")]
static WCG_ACC_CIVACUS: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];
#[cfg(feature = "wcg-paygo")]
static WCG_ACC_AFTERUS: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];
#[cfg(feature = "wcg-paygo")]
static WCG_ACC_RBUS: [AtomicU64; IDS] = [const { AtomicU64::new(0) }; IDS];

/// WCG-CHUNK — per-id: the byte offset [`on_present`]'s `app` checksum covered, or `u64::MAX` for
/// the whole surface (the pre-chunk meaning, and the initial state). [`begin`] consults it before
/// arming the `app` leg: a chunk whose band does not match what `on_present` checksummed — a cursor
/// that moved between the present and the blit, or a present that straddled the sample-1 → sample-2
/// boundary — compares two hashes of different bytes, and a divergence there would be a fabricated
/// RACE-PRESENT. The mismatched chunk runs with `own=no`, exactly the reading the paygo
/// threshold-straddle seam already established: `own=yes` on the wire MEANS the `app=` hash is
/// consultable against `blit=`.
#[cfg(feature = "wcg-paygo")]
static APP_OFF: [AtomicU64; IDS] = [const { AtomicU64::new(u64::MAX) }; IDS];

/// PAYGO-TERM — a new tenant landed on this slot: re-arm the terminal. See [`PAYGO_CLOSED`].
///
/// Only that cell — plus the WCG-CHUNK cursor and its telemetry, which travel with the battery they
/// describe: a new tenant's read-back starts at byte 0 and its `chunks=`/`hold_max_us=` must not
/// inherit a predecessor's, and a stale `APP_OFF` would arm the next tenant's `app` leg against a
/// band the previous tenant's present checksummed. The battery (`TAKEN`, `PAYGO_SAID`) and the
/// census (`PAYGO_EMIT`, `PAYGO_DEFERRED`) are deliberately left alone — the first because it is
/// per-slot by design, the second because `emit=` has to stay monotone per id across a recycle.
/// The banked sums (`WCG_ACC_*`) need no reset here — the first chunk of each sample clears them,
/// and a recycle puts the cursor at 0, which IS that condition.
/// WCH-CUSTODY — a new tenant landed on this slot: the MEASUREMENTS travel with the tenant, the
/// budgets and the monotone wires stay with the slot. Called from `wm::create_inner` beside
/// [`paygo_recycle`], under the same table lock.
///
/// What resets is everything the `[wc-g]`/`[wc-h]` rollups REPORT about a window: verdict censuses
/// (`W_*`), present-phase measurements (`H_TORN`, `H_MAXPRES`, `H_MINSPAN`, `H_BANDED`, `H_WHOLE`,
/// `H_DECLINE`, `H_FIXTURE`), the pending sample slot (`H_PEND` and its fields), the app-checksum
/// seam (`APP_CKS`/`APP_SEQ`/`SEEN_SEQ` — a predecessor's published checksum must not fabricate a
/// RACE verdict against a successor's surface), the refresh pacing pair (`H_LASTROLL`/
/// `H_LASTCENSUS`), and above all `H_T0`: `age_ms=` measured a SLOT's age before this function
/// existed, so the seventh tenant of a busy slot reported an age the boot's first tenant earned
/// (verified defect, Fox 2026-08-13).
///
/// What deliberately does NOT reset, each per its own documented rule: `TAKEN`/`H_TAKEN`/`H_BTAKEN`
/// (sample budgets are per boot — resetting them would unbound the serial spend), `H_EMIT`/`H_LINES`
/// (the reader's "greatest `emit=` supersedes" rule needs monotonicity across recycle), and
/// `H_ROLLED` (the rollup latch rides the budget it latches on).
///
/// Racing composites: a pass that snapshotted the DEAD tenant's row can still fold one sample in
/// after this reset — the same one-fold exposure [`paygo_recycle`] already accepts, and strictly
/// smaller than the whole-life inheritance this function removes. Stated honestly per cell class
/// (review, 2026-08-18): for the COUNTERS the stray is one misattributed count; for the EXTREMA
/// (`H_MAXPRES`/`H_MINPRES`/`H_MINRATE`/`H_MAXRATE`/`H_MINSPAN`, all `fetch_min`/`fetch_max`) a
/// stray fold PERSISTS until the next recycle — including a foreign fast rate seeding
/// [`H_STALL`]'s floor low. Accepted because the race needs a destroy+create of the same slot
/// while a composite is mid-flight on the dead row; the structural fix (a per-row generation
/// plumbed into `stage_note`) is recorded as owed, not taken here. The stall-guard FORBID in
/// x86-witness.spec is bounded at >= 2 for exactly this stray (see the spec's WCH-STALL block).
pub(super) fn wch_recycle(i: usize) {
    if i >= IDS {
        return;
    }
    APP_CKS[i].store(FNV_BASIS, Ordering::Relaxed);
    APP_SEQ[i].store(0, Ordering::Relaxed);
    SEEN_SEQ[i].store(0, Ordering::Relaxed);
    W_SAMPLES[i].store(0, Ordering::Relaxed);
    W_COHER[i].store(0, Ordering::Relaxed);
    W_RACE[i].store(0, Ordering::Relaxed);
    W_BLIT[i].store(0, Ordering::Relaxed);
    W_CLEAN[i].store(0, Ordering::Relaxed);
    W_SLOW[i].store(0, Ordering::Relaxed);
    W_MAXUS[i].store(0, Ordering::Relaxed);
    W_WITUS[i].store(0, Ordering::Relaxed);
    H_TORN[i].store(0, Ordering::Relaxed);
    H_STALL[i].store(0, Ordering::Relaxed);
    // WCH-LONGPRES — the census, the one-shot latch and the pending hand-off all belong to the DEAD
    // tenant. The latch especially: leaving it set would spend the new tenant's one naming line
    // before it has presented once, and the count would then be the only evidence a stall happened.
    H_LONGPRES[i].store(0, Ordering::Relaxed);
    H_LONGSAID[i].store(0, Ordering::Relaxed);
    H_LONGUS[i].store(0, Ordering::Relaxed);
    H_MAXPRES[i].store(0, Ordering::Relaxed);
    H_MINPRES[i].store(u64::MAX, Ordering::Relaxed);
    H_MINRATE[i].store(u64::MAX, Ordering::Relaxed);
    H_MAXRATE[i].store(0, Ordering::Relaxed);
    H_DECLINE[i].store(0, Ordering::Relaxed);
    H_FIXTURE[i].store(0, Ordering::Relaxed);
    H_PEND[i].store(0, Ordering::Relaxed);
    H_KIND[i].store(0, Ordering::Relaxed);
    H_BAND[i].store(0, Ordering::Relaxed);
    H_SPAN[i].store(0, Ordering::Relaxed);
    H_BOX[i].store(0, Ordering::Relaxed);
    H_BYTES[i].store(0, Ordering::Relaxed);
    H_COMPOSE[i].store(0, Ordering::Relaxed);
    H_PRESENT[i].store(0, Ordering::Relaxed);
    H_RECTSCAN[i].store(0, Ordering::Relaxed);
    H_BANDED[i].store(0, Ordering::Relaxed);
    H_WHOLE[i].store(0, Ordering::Relaxed);
    H_MINSPAN[i].store(u64::MAX, Ordering::Relaxed);
    H_T0[i].store(0, Ordering::Relaxed);
    H_LASTROLL[i].store(0, Ordering::Relaxed);
    H_LASTCENSUS[i].store(0, Ordering::Relaxed);
}

#[cfg(feature = "wcg-paygo")]
pub(super) fn paygo_recycle(i: usize) {
    if i < IDS {
        PAYGO_CLOSED[i].store(0, Ordering::Relaxed);
        WCG_CUR[i].store(0, Ordering::Relaxed);
        WCG_BUSY[i].store(0, Ordering::Relaxed);
        WCG_CHUNKS[i].store(0, Ordering::Relaxed);
        WCG_HOLD_MAX_US[i].store(0, Ordering::Relaxed);
        APP_OFF[i].store(u64::MAX, Ordering::Relaxed);
    }
}

/// PAYGO-TERM — shut the wire without printing: this tenant's battery closed on the ORDINARY path
/// (`state=complete … -> PAID`, emitted by [`paygo_complete`] from the pay-at-close composites), so
/// the terminal has already been spoken and the census must not re-open behind it.
///
/// Today `paygo_flush`'s own `TAKEN >= SAMPLES` gate already silences a paid battery, so this is
/// belt-and-braces — but the two facts are different ("the budget is spent" vs "the window said its
/// last word"), and the one the terminal rests on is this one.
#[cfg(feature = "wcg-paygo")]
pub(super) fn paygo_seal_closed(i: usize) {
    if i < IDS {
        PAYGO_CLOSED[i].store(1, Ordering::Release);
        // The deferred waiting line this window had queued is superseded by its terminal. Left set,
        // it would fire on the next tenant's first flush and read as that tenant's opening line.
        PAYGO_PEND[i].store(0, Ordering::Relaxed);
    }
}

/// PAYGO — may this window open a sample now, and on what terms?
///
/// Sample 1 always opens, immediately, and opens LATTICE-SAMPLED. Samples 2..[`SAMPLES`] open at
/// FULL coverage but only once [`PAYGO_DEFER_MS`] of time SINCE KERNEL ENTRY has passed — entry, not
/// reset, and not a guessed clock rate; see [`since_entry_ms`] for what each of those got wrong — so
/// the battery completes on a live desktop instead of inside the boot burst.
///
/// **The gate sits above the budget test and that placement is the whole design.** A declined blit
/// is one this pass does not SAMPLE — it does not spend budget, it does not print a `[wc-g]` line,
/// and it leaves the window's remaining samples available for the next pass that qualifies. A window
/// that stops presenting before the threshold simply keeps its unspent budget, which is what this
/// module already does with a window that stops compositing ([`census_refresh`]'s corollary), so
/// there is nothing here that can wedge: no wait, no retry, no queue, no core to make progress.
///
/// **And the decline is never silent — nor printed from here.** The census moves at the decline and
/// the LINE is recorded for [`stage_flush`] to emit: this function runs inside [`begin`], between
/// WC-D's frozen reference and the copy that reference describes, and a `serial_println!` from there
/// lands in the console window's own surface. See [`PAYGO_PEND`] for the full argument and
/// [`PAYGO_EMIT`] for why that line is then re-emitted on a cadence rather than printed once.
#[cfg(feature = "wcg-paygo")]
fn paygo_open(_id: u32, i: usize) -> bool {
    if TAKEN[i].load(Ordering::Relaxed) == 0 {
        PAYGO_STEP[i].store(PAYGO_LATTICE_N as u32, Ordering::Relaxed);
        return true;
    }
    if !paygo_arm(i) {
        // Census first, unbudgeted, before any print test — WC-H2's rule. Then RECORD the line and
        // let `stage_flush` print it: nothing in this function may write to the UART, because it runs
        // between WC-D's frozen reference and the copy that reference describes. See [`PAYGO_PEND`].
        PAYGO_DEFERRED[i].fetch_add(1, Ordering::Relaxed);
        if PAYGO_SAID[i].swap(1, Ordering::Relaxed) == 0 {
            PAYGO_PEND[i].store(1, Ordering::Release);
        }
        return false;
    }
    PAYGO_STEP[i].store(1, Ordering::Relaxed);
    true
}

/// PAYGO — is this window's next sample payable RIGHT NOW? The ONE definition of the deferral test,
/// so [`begin`]'s gate and [`on_present`]'s cannot drift apart.
///
/// **Why `on_present` has to ask it at all, and what it would cost not to.** `on_present` takes a
/// full-surface checksum on EVERY present while [`budget_left`] is true, which is the arrangement
/// that keeps the `app` leg fresh for whichever blit samples next. Its cheapness rests entirely on
/// the budget spending FAST: four presents and the window goes quiet. Deferral breaks exactly that
/// assumption — a window sits at one spent sample for the whole deferral window — so an `on_present`
/// that ignored PAYGO would checksum the surface on every present for fifteen seconds. On the
/// console window that is megabytes of byte-at-a-time FNV per present at frame rate, which would
/// have cost more than the sampling saves and moved the boot cost rather than removing it. The
/// deferral gate therefore governs both ends of the sample, not just the blit end.
///
/// **And the `app` leg stays live.** The predicate is the same on both ends, so the first present
/// after the threshold checksums, and the blit that follows it opens the sample with a FRESH
/// `cks_app` and a correctly incremented [`APP_SEQ`] — `own=yes` and the RACE-PRESENT leg armed,
/// exactly as without the knob. The one seam is a present that straddles the threshold between the
/// two calls: `on_present` skipped, `begin` opens. Then [`APP_SEQ`] did not move, `own` is false, and
/// the `app` leg is simply not consulted for that one sample — the same thing that already happens on
/// every collateral repaint. It cannot manufacture a verdict, only decline to add one, and the next
/// present is unaffected.
#[cfg(feature = "wcg-paygo")]
#[inline]
fn paygo_arm(i: usize) -> bool {
    if TAKEN[i].load(Ordering::Relaxed) == 0 {
        return true;
    }
    // PAYGO-TERM/PAY-AT-CLOSE — this window is being torn down with its battery still owed, and the
    // deferral has run out of future to be deferred into. See [`PAYGO_FORCE`].
    if PAYGO_FORCE[i].load(Ordering::Relaxed) != 0 {
        return true;
    }
    // An unarmed clock — no entry stamp yet, or no calibrated rate — DEFERS. See [`since_entry_ms`]
    // for why that is the conservative direction and what guessing instead would have cost.
    paygo_clock().2
}

#[cfg(not(feature = "wcg-paygo"))]
#[inline]
fn paygo_arm(_i: usize) -> bool {
    true
}

/// PAYGO-TERM — per-id: pay this window's remaining samples NOW, whatever the deferral clock says.
///
/// **The one place the threshold is overridden, and it is overridden by an event the threshold cannot
/// see.** [`PAYGO_DEFER_MS`] answers "is the boot burst over"; the answer it gives an ORDINARY window
/// is "not yet, wait". A window being CLOSED has no later to wait for — its surface is about to be
/// unmapped and its slot recycled — so the deferral's own argument ("a window that stops presenting
/// keeps its unspent budget, which costs nothing and is reported as unspent") stops holding: what it
/// keeps is not unspent budget but an unterminated battery, printed as `state=waiting` and then
/// silence. Boot V is that case with numbers: `win=4` closed at 13 767 ms with `deferred=1` pending
/// and never said another word.
///
/// Set by `video::wm`'s pay-at-close and CLEARED by it before the row is freed — see there for the
/// cost bound and for why the alternative (seal the window and print `state=sealed -> UNPAID`) was
/// rejected. Never set by anything periodic: a mature deferral is taken by the service-pass taker on
/// the ordinary path, where the clock has genuinely opened and no override is involved.
#[cfg(feature = "wcg-paygo")]
static PAYGO_FORCE: [AtomicU32; IDS] = [const { AtomicU32::new(0) }; IDS];

/// PAYGO-TERM — does this window owe a deferred sample, i.e. was it DECLINED by the gate and is its
/// battery still open? Says nothing about whether the deferral has matured; see [`paygo_ripe`].
///
/// `PAYGO_SAID` and not merely `TAKEN < SAMPLES` is the load-bearing part. A window that simply
/// stopped presenting mid-battery keeps its unspent budget BY DESIGN — that is this module's standing
/// behaviour and predates the deferral entirely — and forcing samples onto it would be inventing
/// composites for a window nobody is drawing. Only a window the DEFERRAL GATE turned away has an
/// obligation this arc created, and only those are taken.
///
/// WCG-CHUNK — **and a PART-PAID sample is pending too, `SAID` or not** (the same M1 fix
/// `wm::wcd_pending` took). A window's FOURTH sample spends `TAKEN` at its first chunk, so a battery
/// parked mid-box on sample 4 reads `TAKEN == SAMPLES`; and a window launched past the deferral
/// threshold is never declined, so `SAID` can be 0 with a cursor mid-box. Either shape was invisible
/// to the 4 Hz taker and to `paygo_at_close`'s UNSPENT terminal — no DEFERRED, no PAID, no UNSPENT,
/// the forbidden silent close. `WCG_CUR != 0` is precisely "a chunk banked without closing", so the
/// taker's whole-box mark now reaches every part-paid sample and drives its cursor home.
#[cfg(feature = "wcg-paygo")]
pub(super) fn paygo_pending(i: usize) -> bool {
    i < IDS
        && ((PAYGO_SAID[i].load(Ordering::Relaxed) != 0
            && TAKEN[i].load(Ordering::Relaxed) < SAMPLES)
            || WCG_CUR[i].load(Ordering::Relaxed) != 0)
}

/// PAYGO-TERM — is this window's deferred sample owed AND payable right now? The service-pass taker's
/// whole predicate, read from the same [`paygo_clock`] the gate defers on so the taker cannot open a
/// sample the gate would decline (which would spin: mark damaged, composite, get declined, repeat).
#[cfg(feature = "wcg-paygo")]
pub(super) fn paygo_ripe(i: usize) -> bool {
    paygo_pending(i) && paygo_clock().2
}

/// PAYGO-TERM — arm/disarm the pay-at-close override. `video::wm::close` is the only caller and it
/// pairs every `true` with a `false`; leaving one set would hand the slot's next tenant a battery
/// with no deferral at all.
#[cfg(feature = "wcg-paygo")]
pub(super) fn paygo_force(i: usize, on: bool) {
    if i < IDS {
        PAYGO_FORCE[i].store(u32::from(on), Ordering::Relaxed);
    }
}

/// Knob off, or not x86: every sample opens exactly as it always did. Folds away entirely.
#[cfg(not(feature = "wcg-paygo"))]
#[inline]
fn paygo_open(_id: u32, _i: usize) -> bool {
    true
}

/// PAYGO — the probe step in force for the sample [`end`] is closing. Constant 1 without the knob,
/// which is what makes [`readback`]'s lattice arithmetic disappear from every other build.
#[cfg(feature = "wcg-paygo")]
#[inline]
fn probe_step(i: usize) -> usize {
    PAYGO_STEP[i].load(Ordering::Relaxed) as usize
}

#[cfg(not(feature = "wcg-paygo"))]
#[inline]
fn probe_step(_i: usize) -> usize {
    1
}

/// PAYGO — what the sample line says about its own coverage, inserted between `fbbad=` and `us=`.
///
/// **A sampled pass must never print a bare `fbbad=0/…` that reads as a full clearance.** `checked`
/// has always been the honest denominator, but a denominator alone does not say WHY it is small —
/// a short window and a lattice pass over a large one can print the same figure. `coverage=` says
/// which, on the line, positionally, where the number it qualifies is.
///
/// A knob-on build marks EVERY sample line, `full` as well as `lattice16`. Marking only the sampled
/// passes would make the marker's absence load-bearing, and an absent field is the one thing a
/// reader cannot distinguish from a field they forgot to look for. A knob-off build inserts the
/// empty string and its lines are byte-identical to the ones this module printed before M3, which is
/// what keeps the aarch64 wire — and the pi4 gate reading it — untouched. ARCH-PARITY (rmbp-7): the
/// KNOB is what keeps it untouched, and since the port that is the only thing that does. No aarch64
/// leg names `wcg-paygo`, so the aarch64 wire is unmoved today; arming it there moves that wire and
/// is the pi4 spec's business to rule on.
#[cfg(feature = "wcg-paygo")]
#[inline]
fn coverage_note(step: usize) -> &'static str {
    if step > 1 {
        " coverage=lattice16"
    } else {
        " coverage=full"
    }
}

#[cfg(not(feature = "wcg-paygo"))]
#[inline]
fn coverage_note(_step: usize) -> &'static str {
    ""
}

/// GR21/WCD-OCC — the `occluded=N occ=n0/n1` field, inserted between `coverage=` and `us=`. A Display
/// shim rather than a `&str` because the values are dynamic, mirroring `wm::BandFmt`; a zero-cost
/// verdict pays no allocation it could fail. It prints the probes a higher window owned and the two
/// snapshot box-counts (pre-blit / read-back), on BOTH arches since PARITY §6.2: the aarch64 blit now
/// withholds occluded pixels too, so a wire that stayed silent about the excuse would be a wire that
/// hid why a probe was not charged. The insertion sits between `coverage=` and `us=`, inside the
/// `.*` of `pi4-regression.spec`'s `[wc-g]` pattern. See [`super::wm::OccSnap`].
struct OccNote {
    occluded: usize,
    n0: usize,
    n1: usize,
}

impl core::fmt::Display for OccNote {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, " occluded={} occ={}/{}", self.occluded, self.n0, self.n1)
    }
}

/// WCG-CHUNK — the ` band=r0..r1` SUFFIX a chunk-local exceptional line carries AFTER its terminal
/// verdict, naming the source rows the convicting chunk actually walked. A suffix and not an
/// insertion, per the wc-d B1 rule: the pi4 gate's sample pattern ends at `->` and the bench
/// analyzer's `WCG_PASS_RE` matches only the line's head, so a suffix is invisible to both, while
/// the `-> COHER`/`-> RACE`/`-> BLIT` FORBIDs (`.*->` forms, no anchor) still fire through it.
/// `None` writes NOTHING — every cumulative close, every unchunked sample, and every knob-off wire
/// on either arch are byte-identical. A `Display` shim like [`BandFmt`](super::wm) and [`OccNote`], so a
/// zero-cost verdict pays no allocation it could fail.
struct BandNote(Option<(usize, usize)>);

impl core::fmt::Display for BandNote {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            Some((r0, r1)) => write!(f, " band={}..{}", r0, r1),
            None => Ok(()),
        }
    }
}

/// PAYGO — the rollup's policy marker, inserted after `scope=window`. The pi4 gate matches
/// `scope=window ` with a trailing space so its pattern cannot match `scope=window-band`; an
/// insertion after the key preserves that, and the empty string preserves the line exactly.
#[cfg(feature = "wcg-paygo")]
const PAYGO_ROLLUP_NOTE: &str = " paygo=yes";

#[cfg(not(feature = "wcg-paygo"))]
const PAYGO_ROLLUP_NOTE: &str = "";

/// PAYGO — the policy's own line: what the sampling regime is, how much it has declined, and when
/// the deferred half becomes payable.
///
/// It is a separate line rather than fields on the sample line for the reason M1 gave for `prof`:
/// the sample line's key order and terminal verdict are matched by another platform track's gate,
/// and a field that track does not read is not worth a chance of breaking it. It fires exactly twice
/// per window at most — once when the deferral starts (`state=waiting`), once when the battery
/// completes (`state=complete`) — so it is bounded without needing a budget of its own.
/// WCG-CHUNK — `chunks` rides only the COMPLETE line ([`paygo_complete`] passes `Some`): the pair
/// is meaningless before the battery closes, and it goes AFTER the `-> PAID` terminal, the suffix
/// position `wm::wcd_paygo_note`'s B1 review established — the bench serial-analyzer's `PAYGO_RE`
/// matches `clock=(\w+) taken=(\d+)` CONTIGUOUSLY with no `$` anchor, and the x86-witness gate's
/// PAID REQUIRE ends at `-> PAID`, so a suffix is invisible to both existing consumers where an
/// insertion between matched keys broke the analyzer's PAID accounting on the wc-d side.
#[cfg(feature = "wcg-paygo")]
fn paygo_note(id: u32, i: usize, state: &str, verdict: &str, chunks: Option<(u32, u64)>) {
    let deferred = PAYGO_DEFERRED[i].load(Ordering::Relaxed);
    let emit = PAYGO_EMIT[i].fetch_add(1, Ordering::Relaxed) + 1;
    // `clock=` disambiguates a real zero from an absent one. `since_entry_ms=0 clock=unarmed` says
    // the entry stamp or the TSC calibration was not there to measure against — which is the state
    // the gate DEFERS in — where `since_entry_ms=0 clock=entry` would be a genuine reading taken at
    // entry. A fabricated zero that could mean either is the kind of field this module keeps
    // convicting.
    let (since_ms, clock, _) = paygo_clock();
    match chunks {
        None => serial_println!(
            "[wc-g] paygo win={} state={} emit={} lattice_n={} deferred={} defer_ms={} since_entry_ms={} clock={} taken={} budget={} -> {}",
            id,
            state,
            emit,
            PAYGO_LATTICE_N,
            deferred,
            PAYGO_DEFER_MS,
            since_ms,
            clock,
            TAKEN[i].load(Ordering::Relaxed).min(SAMPLES),
            SAMPLES,
            verdict
        ),
        Some((n, hold_max_us)) => serial_println!(
            "[wc-g] paygo win={} state={} emit={} lattice_n={} deferred={} defer_ms={} since_entry_ms={} clock={} taken={} budget={} -> {} chunks={} hold_max_us={}",
            id,
            state,
            emit,
            PAYGO_LATTICE_N,
            deferred,
            PAYGO_DEFER_MS,
            since_ms,
            clock,
            TAKEN[i].load(Ordering::Relaxed).min(SAMPLES),
            SAMPLES,
            verdict,
            n,
            hold_max_us
        ),
    }
    // Re-arm the refresh from AFTER the serial write, so `CENSUS_PERIOD_US` bounds the duty cycle
    // this instrument imposes on the composite path and not merely the gap between line starts —
    // the same accounting `stage_rollup` uses. Both stores happen on EVERY emission, including the
    // first and the terminal one, so the cadence is measured from the window's own last line.
    PAYGO_LASTCENSUS[i].store(PAYGO_DEFERRED[i].load(Ordering::Relaxed), Ordering::Relaxed);
    PAYGO_LASTROLL[i].store(now_cycles(), Ordering::Relaxed);
}

/// PAYGO — print the owed `state=waiting` line, or keep a still-deferring window's census current.
///
/// Called from [`stage_flush`], which `video::wm`'s composite pass calls once per window per pass —
/// so this runs on exactly the population it reports on, needs no timer, no thread and no new call
/// site, and a window that stops compositing stops refreshing. That last part is not a gap: its final
/// line describes its last active state and `since_entry_ms=` says when that was.
///
/// The two gates below are [`census_refresh`]'s, for the same reasons: the DELTA gate keeps an idle
/// window silent, and the RATE gate plus the `compare_exchange` keep the cost bounded and let exactly
/// one core print when two flush the same window at once.
#[cfg(feature = "wcg-paygo")]
fn paygo_flush(id: u32, i: usize) {
    // THE TERMINAL IS THE LAST WORD, and this is the check that makes that true. Everything below —
    // the RACE-PRESENT pend, the delta gate, the cadence gate — describes a battery that is still
    // waiting for something, and a closed one is not. Without it a `state=closed … -> UNSPENT` is
    // followed by `state=waiting … -> DEFERRED` at a higher `emit=`, and the module's own reader rule
    // (greatest `emit=` supersedes) then reads the terminal as superseded. See [`PAYGO_CLOSED`].
    if PAYGO_CLOSED[i].load(Ordering::Acquire) != 0 {
        return;
    }
    if PAYGO_PEND[i].swap(0, Ordering::AcqRel) != 0 {
        paygo_note(id, i, "waiting", "DEFERRED", None);
        return;
    }
    // Only while this window is actually deferring. Nothing before its first decline, and nothing
    // after its battery completes — `state=complete` is that window's terminal paygo line.
    if PAYGO_SAID[i].load(Ordering::Relaxed) == 0 || TAKEN[i].load(Ordering::Relaxed) >= SAMPLES {
        return;
    }
    if PAYGO_DEFERRED[i].load(Ordering::Relaxed) == PAYGO_LASTCENSUS[i].load(Ordering::Relaxed) {
        return;
    }
    let last = PAYGO_LASTROLL[i].load(Ordering::Relaxed);
    let now = now_cycles();
    if cycles_to_us(now.saturating_sub(last)) < CENSUS_PERIOD_US {
        return;
    }
    if PAYGO_LASTROLL[i].compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed).is_err() {
        return;
    }
    paygo_note(id, i, "waiting", "DEFERRED", None);
}

#[cfg(not(feature = "wcg-paygo"))]
#[inline]
fn paygo_flush(_id: u32, _i: usize) {}

/// PAYGO — the closing half of [`paygo_note`], emitted beside the rollup so the `deferred=` census
/// is read at the moment the battery is claimed complete rather than only at the moment it began
/// waiting. `deferred=0` here says the deferral gate never declined this window at all.
#[cfg(feature = "wcg-paygo")]
#[inline]
fn paygo_complete(id: u32, i: usize) {
    // WCG-CHUNK — the falsifier rides the terminal: how many chunks the battery's full samples took
    // and the worst single gate-held witness span any of them imposed. See [`WCG_CHUNKS`].
    let chunks = WCG_CHUNKS[i].load(Ordering::Relaxed);
    let hold = WCG_HOLD_MAX_US[i].load(Ordering::Relaxed);
    paygo_note(id, i, "complete", "PAID", Some((chunks, hold)));
}

/// PAYGO-TERM — the OTHER terminal: this window was closed with its battery still owed, and the
/// deferral had not matured, so nothing was bought and nothing will be.
///
/// **Why this is a line and not a payment.** `video::wm`'s pay-at-close DOES run the full battery for
/// a window that dies past [`PAYGO_DEFER_MS`] — that work was already owed and the service-pass taker
/// would have done it moments later. A window that dies BEFORE the threshold is the case this whole
/// module exists for: Boot V closed `win=3`/`win=4`/`win=5` at 13.6–13.8 s, inside the boot burst, and
/// their remaining coverage measures (from Boot V's own `prof` lines, 1.66 us/probe against the
/// uncached panel) at ~27 ms per full `[wc-g]` sample and ~1.5 s for one full `[wc-d]` verdict on a
/// 768x768 rect. Paying that at close would put seconds back onto the boot GR17 took them off — it
/// would be the deferral gate defeated by its own teardown path. So the budget stays unspent and the
/// wire SAYS SO.
///
/// **And it is deliberately not `-> UNPAID`.** That token belongs to `wm`'s teardown-interlock abort:
/// a window whose read-back was overwritten under it, sealed after exhausting `WCD_ABORT_MAX`, which
/// x86-witness.spec FORBIDs outright because it is a real defect. This is not that. `-> UNSPENT` says
/// the budget was never spent and no verdict was owed — the designed steady state of a short-lived
/// window — and it matches no directive in any spec, which is correct: there is nothing here to
/// require and nothing to forbid.
///
/// **The latch is taken BEFORE the print, and that ordering is the whole of §3c's fix.** `emit=` is
/// stamped inside [`paygo_note`], so shutting the wire first means every lane that reaches
/// [`paygo_flush`]'s gate from this instant on declines to emit and the terminal carries the greatest
/// `emit=` this tenant will ever produce — which is exactly what the reader's supersession rule needs
/// to read `closed` as the final state. The `swap` also makes the line idempotent: a second closer of
/// the same tenant (there is none today; the row is freed under the caller) prints nothing.
#[cfg(feature = "wcg-paygo")]
pub(super) fn paygo_closed(id: u32, i: usize) {
    if i >= IDS {
        return;
    }
    // The queued RACE-PRESENT line is superseded too: it says "waiting", and this window is not.
    PAYGO_PEND[i].store(0, Ordering::Relaxed);
    if PAYGO_CLOSED[i].swap(1, Ordering::AcqRel) != 0 {
        return;
    }
    paygo_note(id, i, "closed", "UNSPENT", None);
}

#[cfg(not(feature = "wcg-paygo"))]
#[inline]
fn paygo_complete(_id: u32, _i: usize) {}

/// Open a sample: take the `blit` checksum (the surface exactly as `draw_window` is about to read
/// it), then the `civac` checksum (the same bytes through the coherent view), and start the clock.
///
/// Returns `None` when this window is out of budget, is a compat row (chrome-less legacy shim — not
/// the path under investigation), or has no mapped surface. Call [`end`] with the returned probe
/// immediately after `draw_window` returns, and with nothing in between: the `after` checksum's
/// meaning is "the surface as it stood when the copy finished", and any work inserted between the
/// two widens the window it measures.
///
/// ### OCC62 M1 — the pre-blit excuse is NOT carried in the [`Probe`], and that is a stack budget
///
/// It used to be: `begin` took an [`super::wm::OccSnap`] BY VALUE and stored it, so every probe
/// alive across `draw_window` pinned 392 bytes (`MAX_WINDOWS * 32 + 8`) of the compositor's frame.
/// Ported to aarch64 unchanged, that copy — beside the one in `wm::VerifyRef`, the two argument
/// temporaries, and `verify_window`'s own — exhausted the kernel task stack and faulted the armed
/// bench-geometry boot at the shell-window create (`=== AARCH64 EXCEPTION` after
/// `[wc-a] create win=3 … 960x583`, A/B'd against the same event on the base sha, which survives it).
///
/// So the snapshot stays OWNED by the caller's per-window frame — ONE instance, shared by this
/// bracket and by WC-D's reference — and [`end`] borrows it. Nothing about the excuse's MEANING
/// moved: it is still the set as of the blit, taken once, and `occ=n0/` still prints its count.
pub fn begin(
    id: u32,
    surf: usize,
    surf_len: usize,
    compat: bool,
) -> Option<Probe> {
    let i = id as usize; seam_register_once(); // WINID2 (SO1(b) / A29) — ⚠ SAME-LINE fold, line-NEUTRAL. `SEAM_WIN` is the SIXTH id cache the WINID block's table of five missed (rmbp 13's sweep for the shape), and it is cleared on no path at all. It cannot be registered on its own store line the way the other five are — that line is inside `seam_glyph_note`, which runs once per GLYPH in print context under this file's own "never take a lock, allocate, or print" contract — so it is registered HERE instead: `begin` STRICTLY DOMINATES both readers (`end` adjudicates only a `Probe` this call handed out), it is off the print path, and it is ahead of the first timed span in this function, so no measured bracket widens. Boot-once latched, so the registry mutex is taken exactly once. See the WINID2 block at this file's tail.
    if compat || surf == 0 || surf_len == 0 || i >= IDS {
        return None;
    }
    // WCG-CHUNK — is this admission a CHUNK of a full-coverage sample? `TAKEN > 0` means the next
    // sample is a full-coverage one (sample 1 is the lattice), and `WCG_CUR > 0` means one is
    // already mid-box; either way the single-walker latch is claimed FIRST, before any budget is
    // spent, so a losing lane declines without a spend to unwind. A fresh full sample (cursor at 0)
    // still runs the deferral gate and the budget spend exactly as before — the deferral census,
    // `state=waiting -> DEFERRED`, and the saturate law are all unchanged — and a RESUMED chunk
    // bypasses both, because its sample was admitted and paid for at its first chunk.
    #[cfg(feature = "wcg-paygo")]
    let (chunk, band_off, band_len) = {
        let cur = WCG_CUR[i].load(Ordering::Relaxed) as usize;
        if cur > 0 || TAKEN[i].load(Ordering::Relaxed) > 0 {
            if WCG_BUSY[i].swap(1, Ordering::AcqRel) != 0 {
                return None;
            }
            if cur == 0 {
                if !paygo_open(id, i) {
                    WCG_BUSY[i].store(0, Ordering::Release);
                    return None;
                }
                if TAKEN[i].fetch_add(1, Ordering::Relaxed) >= SAMPLES {
                    TAKEN[i].store(SAMPLES, Ordering::Relaxed);
                    WCG_BUSY[i].store(0, Ordering::Release);
                    return None;
                }
            }
            let lo = cur.min(surf_len);
            let hi = cur.saturating_add(WCG_CHUNK_BYTES).min(surf_len);
            (true, lo, hi.saturating_sub(lo))
        } else {
            // Sample 1, the lattice: admitted and bracketed whole, exactly as before.
            if !paygo_open(id, i) {
                return None;
            }
            if TAKEN[i].fetch_add(1, Ordering::Relaxed) >= SAMPLES {
                TAKEN[i].store(SAMPLES, Ordering::Relaxed);
                return None;
            }
            (false, 0, surf_len)
        }
    };
    // WC-G/M3 — PAYGO's coverage gate, and it must stay ABOVE the budget test: a deferred blit is
    // one this pass declines to SAMPLE, not one it spends a sample on. Compiles to `true` and folds
    // away whenever the knob is off or the arch is not x86. See [`paygo_open`]. (On the `wcg-paygo`
    // build both gates run inside the WCG-CHUNK admission above instead, in the same order.)
    #[cfg(not(feature = "wcg-paygo"))]
    {
        if !paygo_open(id, i) {
            return None;
        }
        if TAKEN[i].fetch_add(1, Ordering::Relaxed) >= SAMPLES {
            // Saturate rather than wrap: the counter is also the budget test in `budget_left`.
            TAKEN[i].store(SAMPLES, Ordering::Relaxed);
            return None;
        }
    }
    // WCG-CHUNK — what the checksum trio walks: the chunk's band, or the whole surface on every
    // path that existed before the chunking (band_off = 0, band_len = surf_len there).
    #[cfg(feature = "wcg-paygo")]
    let (cs_at, cs_len) = (surf + band_off, band_len);
    #[cfg(not(feature = "wcg-paygo"))]
    let (cs_at, cs_len) = (surf, surf_len);
    // WC-G/M1 — phase timestamps around the EXISTING operations, all of them BEFORE `t0` is set.
    // Four clock reads on a path that already does two full-surface volatile reads is arithmetic
    // next to the work being measured, and — this is the load-bearing part — none of it lands
    // between the `t0` assignment and the return, so the ordering law below is untouched and `us=`
    // still contains the copy and nothing else.
    // WCGSEAM — the census snapshot this sample's bracket opens on. One relaxed load, above the
    // checksums and therefore above `t0`, per the ordering law on the literal below.
    let seam0 = SEAM_WRITES.load(Ordering::Relaxed);
    let tp0 = now_cycles();
    let cks_blit = checksum(cs_at, cs_len);
    let tp1 = now_cycles();
    // The coherency leg. See the module note on why this cleans as well as invalidates.
    clean_invalidate_surface(cs_at, cs_len);
    let cks_civac = checksum(cs_at, cs_len);
    let tp2 = now_cycles();
    let cks_blit_us = cycles_to_us(tp1.saturating_sub(tp0));
    let civac_us = cycles_to_us(tp2.saturating_sub(tp1));

    let seq = APP_SEQ[i].load(Ordering::Relaxed);
    let own = SEEN_SEQ[i].swap(seq, Ordering::Relaxed) != seq;
    // WCG-CHUNK — the `app` leg is consultable only if [`on_present`] checksummed the SAME bytes
    // this probe's `blit` leg walked; otherwise the chunk runs `own=no` — the reading the paygo
    // threshold-straddle seam already established, and the guard that keeps a hash comparison of
    // DIFFERENT byte ranges from fabricating a RACE-PRESENT. See [`APP_OFF`].
    #[cfg(feature = "wcg-paygo")]
    let own = own
        && APP_OFF[i].load(Ordering::Relaxed)
            == if chunk { band_off as u64 } else { u64::MAX };

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
        // WCGSEAM — a plain move, above `t0` for the same reason.
        seam0,
        // WCG-CHUNK — three plain moves, above `t0` for the same reason.
        #[cfg(feature = "wcg-paygo")]
        chunk,
        #[cfg(feature = "wcg-paygo")]
        band_off,
        #[cfg(feature = "wcg-paygo")]
        band_len,
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
pub fn end(
    p: Probe,
    fb: &FrameBuffer,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    stride: usize,
    scale: usize,
    occ_before: &super::wm::OccSnap,
    occ_after: &super::wm::OccSnap,
) {
    let us = cycles_to_us(now_cycles().saturating_sub(p.t0));
    // WCG-CHUNK — the `after` leg walks the SAME bytes the `blit`/`civac` legs walked in [`begin`]:
    // the chunk's band, or the whole surface on every pre-chunk path (`band_off = 0`,
    // `band_len = surf_len` there).
    #[cfg(feature = "wcg-paygo")]
    let (cs_at, cs_len) = (p.surf + p.band_off, p.band_len);
    #[cfg(not(feature = "wcg-paygo"))]
    let (cs_at, cs_len) = (p.surf, p.surf_len);
    // WC-G/M1 — `us=` above is computed FIRST and from `p.t0` alone, exactly as it always was. Every
    // phase clock below starts after it, so no profiling read can enter the timing verdict's bracket.
    let tp0 = now_cycles();
    let cks_after = checksum(cs_at, cs_len);
    let tp1 = now_cycles();
    let cks_after_us = cycles_to_us(tp1.saturating_sub(tp0));
    // WCGSEAM — the census is read HERE, immediately after the `after` leg and outside every
    // bracket: the delta against `p.seam0` then covers exactly the span the checksum trio
    // adjudicates over (blit → civac → after), which is the span a COHER or RACE-BLIT verdict
    // convicts. Two relaxed loads; the line they may feed prints far below, after the sample line.
    let seam_now = SEAM_WRITES.load(Ordering::Relaxed);
    let seam_last = SEAM_LAST_CYC.load(Ordering::Relaxed);

    // Re-derive the destination from the source, one probe per SOURCE pixel (the top-left
    // destination pixel of each upscale cell). Bounds mirror `draw_window`'s: the panel clip, the
    // stride column bound, and the `surf_len` row bound. WC-G/M3 moved the walk itself into
    // [`readback`] so the two things that reshape it — the wide glass read and PAYGO's lattice —
    // live where they can be read together, and so this function's brackets stayed where they are.
    // WC-G/M3 — the panel geometry is read HERE, above the bracket, and passed down. `readback` used
    // to call `fb.info()` itself, which put that read inside the `readback_us` clock; the bracket law
    // in this module is that a bracket contains the operation it names and nothing else, and `ph` is
    // needed below for `rectscan_us` regardless.
    let info = fb.info();
    let (pw, ph) = (info.width, info.height);
    // WC-G/M3 — the terms this sample was admitted on. Read OUTSIDE the bracket below: it is one
    // relaxed load, but the bracket's job is to contain the walk and nothing else.
    let step = probe_step(p.id as usize);
    // WCG-CHUNK — the chunk's row window, derived from the band the checksums bracketed. The band
    // offset is a whole-row multiple of the stride the ADVANCING chunk saw, so a non-multiple here
    // means the geometry changed under a part-paid sample; the walk is declined (`row0 = cap`, an
    // empty window) and the banking block below closes the sample `coverage=shrunk` rather than
    // comparing rows the band never covered. `.max(row0 + 1)` is the "whole rows, at least one" law
    // — unreachable while [`WCG_CHUNK_BYTES`] covers any real panel's stride, and stated anyway.
    #[cfg(feature = "wcg-paygo")]
    let (rb_row0, rb_cap, rb_bounded) = if p.chunk {
        if stride >= 4 && p.band_off % stride == 0 {
            let row0 = p.band_off / stride;
            (row0, ((p.band_off + p.band_len) / stride).max(row0 + 1), true)
        } else {
            (usize::MAX, usize::MAX, true)
        }
    } else {
        (0, usize::MAX, false)
    };
    #[cfg(not(feature = "wcg-paygo"))]
    let (rb_row0, rb_cap, rb_bounded) = (0usize, usize::MAX, false);
    // WC-G/M1 — the read-back's own bracket. On x86 the glass is WC-mapped PCIe memory and every
    // probe is an uncached round trip to the device, so this walk is the phase most likely to
    // dominate the pass — which is exactly why it could not go on being reported as part of an
    // undifferentiated total. The bracket wraps the walk and nothing else, so the clock contains no
    // formatting and no serial. (M3 narrowed what a probe COSTS and, under PAYGO, how many of them a
    // pass takes; it did not move this bracket or change what it contains.)
    let tp2 = now_cycles();
    // `step_eff` is what the walk ACTUALLY used — `readback` collapses the lattice on a rect narrower
    // than its own step — and it is what the `coverage=` marker below is derived from, so the wire
    // reports the pass that ran rather than the pass that was requested.
    // GR21/WCD-OCC — the read-back excuses probes a higher window owns and returns their count as a
    // fourth element. PARITY §6.2: one call, both arches.
    let (bad, checked, step_eff, occluded, rows_done, rows_total) = readback(
        fb, p.surf, p.surf_len, pw, ph, x, y, w, h, stride, scale, step, rb_row0, rb_cap,
        rb_bounded, occ_before, occ_after,
    );
    let readback_us = cycles_to_us(now_cycles().saturating_sub(tp2));
    #[cfg(not(feature = "wcg-paygo"))]
    let _ = (rows_done, rows_total);
    // WCGSEAM-HB — the honest bracket's SECOND census read, the instant the read-back bracket
    // closes. The refund test below must cover the FULL adjudication span — blit → civac → after →
    // read-back — because a `BLIT` verdict convicts on `fbbad`, and glyphs stored during the walk
    // (after `seam_now` was taken) are invisible to the checksum-span delta the `[wcgseam]` line
    // prints. One relaxed load, outside every timing bracket.
    #[cfg(not(all(target_arch = "x86_64", feature = "wcg-paygo")))]
    let seam_rb = SEAM_WRITES.load(Ordering::Relaxed);

    // WCG-CHUNK — bank this chunk, and decide whether it CLOSES the sample.
    //
    // A clean chunk that did not reach the box's last row prints NOTHING and returns: the cursor
    // advances, the counts are banked, chunk progress re-arms the service taker's liveness bound
    // (`wm::PAYGO_SVC_TRIES` caps marks WITHOUT progress — its anti-wedge purpose — and a sample
    // that now takes a box in hundreds of chunks would exhaust a fixed cap of 16 while doing
    // exactly what it was asked to), and the next admitted composite resumes the walk. The sample
    // line speaks once, cumulative, when the closing chunk lands — one line per sample on the wire,
    // exactly as before, `coverage=full` because the cursor walked the box contiguously from row 0.
    //
    // An EXCEPTIONAL chunk — a checksum leg diverged (COHER / RACE-BLIT / RACE-PRESENT, all
    // adjudicated over this chunk's own band and bracket) or a chargeable probe (`bad != 0`) —
    // prints immediately, chunk-local, `coverage=band` with ` band=` naming the rows it walked
    // AFTER the terminal verdict (a suffix, invisible to the pi4 gate's `fbbad=.* slow=.* ->`
    // pattern and to the analyzer's `WCG_PASS_RE`, while every `-> COHER`/`RACE`/`BLIT` FORBID
    // still fires through the `.*`). It closes the sample the way one bad verdict always has — the
    // battery keeps its remaining budget, exactly as an unchunked bad sample leaves it.
    //
    // A box that SHRANK (or re-strode) under a part-paid sample closes with the banked sums and
    // `coverage=shrunk` — satisfying no gate's `full` REQUIRE and no FORBID — rather than wedging
    // the cursor past a row extent that no longer exists; every banked chunk was clean, so the
    // verdict those sums support is the one printed.
    #[cfg(feature = "wcg-paygo")]
    let (bad, checked, occluded, us, cov_over, band_note) = if !p.chunk {
        (bad, checked, occluded, us, None, BandNote(None))
    } else {
        let wi = p.id as usize;
        // The falsifier's raw material: this chunk's gate-held witness span, the same four phases
        // `wit_us=` sums. See [`WCG_CHUNKS`] for the span statement.
        let hold = p
            .cks_blit_us
            .saturating_add(p.civac_us)
            .saturating_add(cks_after_us)
            .saturating_add(readback_us);
        WCG_CHUNKS[wi].fetch_add(1, Ordering::Relaxed);
        WCG_HOLD_MAX_US[wi].fetch_max(hold, Ordering::Relaxed);
        if p.band_off == 0 {
            // First chunk of a sample: the banked sums start clean. Single-writer by construction
            // ([`WCG_BUSY`] is held), so plain stores.
            WCG_ACC_CHECKED[wi].store(0, Ordering::Relaxed);
            WCG_ACC_OCC[wi].store(0, Ordering::Relaxed);
            WCG_ACC_USMAX[wi].store(0, Ordering::Relaxed);
            WCG_ACC_BYTES[wi].store(0, Ordering::Relaxed);
            WCG_ACC_BLITUS[wi].store(0, Ordering::Relaxed);
            WCG_ACC_CIVACUS[wi].store(0, Ordering::Relaxed);
            WCG_ACC_AFTERUS[wi].store(0, Ordering::Relaxed);
            WCG_ACC_RBUS[wi].store(0, Ordering::Relaxed);
        }
        let acc_checked =
            WCG_ACC_CHECKED[wi].fetch_add(checked as u64, Ordering::Relaxed) + checked as u64;
        let acc_occ =
            WCG_ACC_OCC[wi].fetch_add(occluded as u64, Ordering::Relaxed) + occluded as u64;
        let acc_us = WCG_ACC_USMAX[wi].fetch_max(us, Ordering::Relaxed).max(us);
        WCG_ACC_BYTES[wi].fetch_add(p.band_len as u64, Ordering::Relaxed);
        WCG_ACC_BLITUS[wi].fetch_add(p.cks_blit_us, Ordering::Relaxed);
        WCG_ACC_CIVACUS[wi].fetch_add(p.civac_us, Ordering::Relaxed);
        WCG_ACC_AFTERUS[wi].fetch_add(cks_after_us, Ordering::Relaxed);
        WCG_ACC_RBUS[wi].fetch_add(readback_us, Ordering::Relaxed);
        // The per-window ledger keeps counting per chunk — a part-paid sample's witness time must
        // not vanish if the box never closes. The unchunked add below is skipped for chunks.
        W_WITUS[wi].fetch_add(hold, Ordering::Relaxed);
        let exceptional = p.cks_blit != p.cks_civac
            || p.cks_blit != cks_after
            || (p.own && p.cks_app != p.cks_blit)
            || bad != 0;
        if exceptional {
            WCG_CUR[wi].store(0, Ordering::Relaxed);
            WCG_BUSY[wi].store(0, Ordering::Release);
            (
                bad,
                checked,
                occluded,
                us,
                Some(" coverage=band"),
                BandNote(Some((rb_row0, rows_done))),
            )
        } else if rb_row0 >= rows_total {
            // Shrunk (or re-strode, `rb_row0 = usize::MAX` above): every row the box still has was
            // walked by the banked chunks. Close, cumulative, coverage named honestly.
            WCG_CUR[wi].store(0, Ordering::Relaxed);
            WCG_BUSY[wi].store(0, Ordering::Release);
            (
                0,
                acc_checked as usize,
                acc_occ as usize,
                acc_us,
                Some(" coverage=shrunk"),
                BandNote(None),
            )
        } else if rows_done >= rows_total {
            // The closing chunk: the cursor reached the box's last row.
            WCG_CUR[wi].store(0, Ordering::Relaxed);
            WCG_BUSY[wi].store(0, Ordering::Release);
            (0, acc_checked as usize, acc_occ as usize, acc_us, Some(" coverage=full"), BandNote(None))
        } else {
            // Silent clean chunk: bank, advance, hand the budget back. No line, no verdict count.
            WCG_CUR[wi].store((rows_done * stride) as u64, Ordering::Relaxed);
            // ARCH-PARITY (rmbp-7, closed by WMPAYGO in the same fold that opened the bootpace
            // hook): the prediction the old comment made here came true — `wm.rs`'s paygo half is
            // ported, the taker/counter/STOP-NOTE ride the feature terms alone, and this call
            // follows the rest of the family across. What it clears is `wm::PAYGO_SVC_TRIES`, the
            // liveness bound of wc-d's service-pass taker, which now exists wherever this caller
            // does. The hook and this progress report moved TOGETHER, deliberately: a taker whose
            // cap fills with no progress able to re-arm it trips its own STOP-NOTE.
            super::wm::paygo_svc_progress(wi);
            WCG_BUSY[wi].store(0, Ordering::Release);
            return;
        }
    };
    #[cfg(not(feature = "wcg-paygo"))]
    let (cov_over, band_note): (Option<&'static str>, BandNote) = (None, BandNote(None));

    // Attribution, most specific first. A source that moved under the copy invalidates the
    // read-back's expectation (it was re-derived from bytes the blit never saw), so RACE outranks
    // BLIT rather than being reported alongside it — `fbbad` is still printed, so the raw number is
    // never hidden by the verdict drawn from it.
    let w = p.id as usize;
    // WCGSEAM-HB — classification is now PURE (no counter moves), because the honest bracket below
    // may decline to adjudicate this sample at all. The counting happens after the refund gate, and
    // exactly once per ADJUDICATED sample, same precedence, same tokens.
    let verdict = if p.cks_blit != p.cks_civac {
        "COHER"
    } else if p.cks_blit != cks_after {
        "RACE-BLIT"
    } else if p.own && p.cks_app != p.cks_blit {
        "RACE-PRESENT"
    } else if bad != 0 {
        "BLIT"
    } else {
        "CLEAN"
    };
    // WCGSEAM-HB — THE HONEST BRACKET: bracket the hash against owner progress. A convicting
    // sample of the ROUTED CONSOLE'S window whose full adjudication span the census marks dirty
    // (`rb_delta > 0`: fbcon's glyph raster stored into the source while this pass was reading it)
    // is REFUNDED — the budget spend is undone so a later present re-arms the sample — instead of
    // adjudicated. The 2026-08-25 bench-geometry reading (header note above) caught the writer
    // inside the bracket on ALL THREE convictions of the armed boot; convicting a source that is
    // known-mutable by design during the boot seam measures the bracket's width, not the
    // compositor's correctness. What this deliberately does NOT do: it never excuses a quiet
    // bracket (metal cache incoherence, a non-fbcon writer, a deterministic blit defect — all still
    // convict, which is what keeps every `-> COHER`/`RACE`/`BLIT` FORBID load-bearing), it never
    // excuses any window but the census's own, it stops excusing after [`REARM_MAX`] refunds, and
    // it hides nothing: the refunded pass prints its `[wcgseam]` line — sole line of the pass, with
    // the verdict it declined to adjudicate and the refund tally as a suffix AFTER the terminal
    // (the standing insertion rule; the adjudicated line's pre-registered grammar is untouched).
    // The refund REDUCES serial per pass (one `[wcgseam]` line instead of sample + prof), and the
    // witness time it spent still lands in `wit_us=` — the cost was paid, so the ledger says so.
    #[cfg(not(all(target_arch = "x86_64", feature = "wcg-paygo")))]
    if verdict != "CLEAN" && p.id != 0 && p.id == SEAM_WIN.load(Ordering::Relaxed) {
        let rb_delta = seam_rb.saturating_sub(p.seam0);
        // Single-compositor-context load/store, like every W_* pattern in this file.
        let used = W_REARM[w].load(Ordering::Relaxed);
        if rb_delta > 0 && used < REARM_MAX {
            W_REARM[w].store(used + 1, Ordering::Relaxed);
            // Undo `begin`'s spend. Admission declines at `>= SAMPLES` post-add, so the counter is
            // in `1..=SAMPLES` here and the sub cannot underflow.
            TAKEN[w].fetch_sub(1, Ordering::Relaxed);
            W_WITUS[w].fetch_add(
                p.cks_blit_us
                    .saturating_add(p.civac_us)
                    .saturating_add(cks_after_us)
                    .saturating_add(readback_us),
                Ordering::Relaxed,
            );
            // PARWCG — RULED: this gate SHOULD be the route's own condition, and the only thing
            // holding it narrow is availability, not meaning.
            //
            // MEANING first, because that is the part that is settled. "Is the console routed right
            // now?" is the routed console's question, not this gate's. The cell the answer is read
            // out of (`fbcon::CONSOLE_WIN`) already carries the ROUTE's own condition, and this
            // witness's own seam census is fed by the routed glyph writes of every build that HAS a
            // route — `seam_glyph_note` is called from the console's paint paths under exactly that
            // condition. So the rollup CAN carry `routed=` truthfully wherever a route can exist,
            // and it is not derivable from the guard above either: reaching this print proves a
            // route existed when the glyphs were charged, never that one is live now (a panic
            // backdrop and a furniture close each clear the cell). Printing `?` where the answer is
            // both knowable and load-bearing is the rollup withholding the one fact the line exists
            // to attribute.
            //
            // AVAILABILITY was what blocked it, and the blocker is gone: PARFB widened the query
            // (`fbcon::console_is_routed`) to the route's own condition, and this fold widens BOTH
            // copies in this file to that identical condition, in lockstep — a `no` printed by one
            // and a `?` by the other on the same boot would read as a route that came and went.
            // The `?` arm remains for builds where no route can exist at all.
            #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
            let routed = if super::fbcon::console_is_routed() { "yes" } else { "no" };
            #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
            let routed = "?";
            serial_println!(
                "[wcgseam] win={} seq={} verdict={} routed={} glyphs={} delta={} locked={} last_age_us={} -> {} rb_delta={} refunded={}/{}",
                p.id,
                p.seq,
                verdict,
                routed,
                seam_now,
                seam_now.saturating_sub(p.seam0),
                SEAM_LOCKED.load(Ordering::Relaxed),
                cycles_to_us(now_cycles().saturating_sub(seam_last)),
                if seam_now > p.seam0 { "GLYPH-RASTER" } else { "QUIET-BRACKET" },
                rb_delta,
                used + 1,
                REARM_MAX
            );
            return;
        }
    }
    match verdict {
        "COHER" => {
            W_COHER[w].fetch_add(1, Ordering::Relaxed);
        }
        "RACE-BLIT" | "RACE-PRESENT" => {
            W_RACE[w].fetch_add(1, Ordering::Relaxed);
        }
        "BLIT" => {
            W_BLIT[w].fetch_add(1, Ordering::Relaxed);
        }
        _ => {
            W_CLEAN[w].fetch_add(1, Ordering::Relaxed);
        }
    }
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

    // GR21/WCD-OCC — the `occluded=`/`occ=` field, built here so the serial_println below stays a
    // single shared call across arches. PARITY §6.2: both arches carry the excused-probe count and the
    // two snapshot box-counts, because both arches now withhold the pixels behind them.
    let occ_note = OccNote { occluded, n0: occ_before.count(), n1: occ_after.count() };

    serial_println!(
        // WC-G/M3 — `coverage=` is an INSERTION between `fbbad=` and `us=`, which is what the pi4
        // gate's `\[wc-g\] win=.* fbbad=.* slow=.* ->` permits: nothing renamed, nothing reordered,
        // the terminal still terminal. It is the empty string on every build but an x86 `wcg-paygo`
        // one, so those lines are byte-identical to the ones this module printed before M3. See
        // [`coverage_note`] for why a sampled pass may not print a bare `fbbad=0/…`.
        // GR21/WCD-OCC — `occluded=`/`occ=` is a second such insertion, in the SAME window (between
        // `coverage=` and `us=`, still inside the pi4 pattern's `.*`). ARCH-PARITY (rmbp-7): it is
        // no longer empty on aarch64 — the attribution `occluded=` reports is now taken on both
        // arches, which is what [`readback`]'s own comment and [`OccNote`]'s doc always claimed.
        // WCG-CHUNK — a chunked sample's closing line overrides the marker (`full`, or the honest
        // `band`/`shrunk` on the exceptional and shrunk closes), and ` band=` rides AFTER the
        // terminal verdict as a suffix, empty everywhere but a chunk-local exceptional line.
        "[wc-g] win={} seq={} own={} scale={}x app={:#018x} blit={:#018x} civac={:#018x} after={:#018x} fbbad={}/{}{}{} us={} rectscan_us={} slow={} -> {}{}",
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
        cov_over.unwrap_or(coverage_note(step_eff)),
        occ_note,
        us,
        rectscan_us,
        if slow { "yes" } else { "no" },
        verdict,
        band_note
    );

    // WCGSEAM — WHO the concurrent writer was, printed beside the conviction it explains.
    //
    // Rides the sample line's budget exactly (reachable only where the line above printed), fires
    // only on a NON-CLEAN verdict, and only for the window fbcon has charged as the routed
    // console's — a conviction on an app window has nothing to learn from a console census, and a
    // boot that never routed the console never prints this at all. `delta=` is the number the two
    // remedy candidates of PARITY §6.13 are decided on: writes that landed INSIDE this sample's
    // adjudication bracket (`-> GLYPH-RASTER`, the writer caught in the act) versus none
    // (`-> QUIET-BRACKET`, with `last_age_us=` saying how far back the last glyph store landed —
    // the COHER shape, where the store precedes the bracket and only its cache residue is caught).
    // `locked=` splits the census by paint path, which is what separates remedy (a) — an FBCON
    // lock, able to serialise only lock-taking writers — from remedy (b), a routed-window decline
    // in `begin`. A new tag deliberately: no pi4 FORBID matches `\[wcgseam\]`, and none may be
    // taught to until the discriminator has spoken on the bench.
    if verdict != "CLEAN" && p.id != 0 && p.id == SEAM_WIN.load(Ordering::Relaxed) {
        // PARWCG — the second of the two copies. Ruling, evidence and the verified successor gate
        // are at the sibling site in the refund arm above; this one carries no separate reasoning
        // and must never acquire any. Widen the two together or not at all.
        #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
        let routed = if super::fbcon::console_is_routed() { "yes" } else { "no" };
        #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
        let routed = "?";
        serial_println!(
            "[wcgseam] win={} seq={} verdict={} routed={} glyphs={} delta={} locked={} last_age_us={} -> {}",
            p.id,
            p.seq,
            verdict,
            routed,
            seam_now,
            seam_now.saturating_sub(p.seam0),
            SEAM_LOCKED.load(Ordering::Relaxed),
            cycles_to_us(now_cycles().saturating_sub(seam_last)),
            if seam_now > p.seam0 { "GLYPH-RASTER" } else { "QUIET-BRACKET" }
        );
    }

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
    // WCG-CHUNK — a chunked sample's `prof` line is CUMULATIVE, like the sample line it rides
    // behind: the four phases sum over every chunk (this one banked itself above), `probes=` is the
    // banked `checked`, and `surf_bytes=` is the bytes the checksum trio actually walked — the
    // banded sums, which land within one band of `surf_len` on a closing chunk and honestly smaller
    // on an exceptional or shrunk one. One `prof` per sample, exactly as before; silent chunks
    // print nothing.
    #[cfg(feature = "wcg-paygo")]
    let (pf_bytes, pf_blit_us, pf_civac_us, pf_after_us, pf_probes, pf_rb_us) = if p.chunk {
        (
            WCG_ACC_BYTES[w].load(Ordering::Relaxed),
            WCG_ACC_BLITUS[w].load(Ordering::Relaxed),
            WCG_ACC_CIVACUS[w].load(Ordering::Relaxed),
            WCG_ACC_AFTERUS[w].load(Ordering::Relaxed),
            WCG_ACC_CHECKED[w].load(Ordering::Relaxed),
            WCG_ACC_RBUS[w].load(Ordering::Relaxed),
        )
    } else {
        (p.surf_len as u64, p.cks_blit_us, p.civac_us, cks_after_us, checked as u64, readback_us)
    };
    #[cfg(not(feature = "wcg-paygo"))]
    let (pf_bytes, pf_blit_us, pf_civac_us, pf_after_us, pf_probes, pf_rb_us) =
        (p.surf_len, p.cks_blit_us, p.civac_us, cks_after_us, checked, readback_us);
    serial_println!(
        "[wc-g] prof win={} seq={} surf_bytes={} cks_blit_us={} civac_us={} cks_after_us={} probes={} readback_us={}",
        p.id,
        p.seq,
        pf_bytes,
        pf_blit_us,
        pf_civac_us,
        pf_after_us,
        pf_probes,
        pf_rb_us
    );
    // The per-window ledger the rollup's `wit_us=` reports. Accumulated AFTER the prints, so a
    // window's total is the sum of the four measured phases and carries no serial time. See
    // [`W_WITUS`]. WCG-CHUNK — a chunk already banked its phases into the ledger above (a part-paid
    // sample's time must not vanish), so it is skipped here.
    #[cfg(feature = "wcg-paygo")]
    let wit_here = !p.chunk;
    #[cfg(not(feature = "wcg-paygo"))]
    let wit_here = true;
    if wit_here {
        W_WITUS[w].fetch_add(
            p.cks_blit_us
                .saturating_add(p.civac_us)
                .saturating_add(cks_after_us)
                .saturating_add(readback_us),
            Ordering::Relaxed,
        );
    }

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
        //
        // WC-G/M3 — the battery this rollup closes was paid in two coverages under PAYGO, and the
        // line that says so goes FIRST so a reader meets the policy before the verdict drawn under
        // it. Nothing on a knob-off build.
        paygo_complete(p.id, w);
        serial_println!(
            // WC-G/M3 — `paygo=yes` is an INSERTION after `scope=window`, and the trailing space the
            // pi4 gate relies on to keep `scope=window ` from matching `scope=window-band` survives
            // it. Empty on every other build.
            "[wc-g] rollup win={} scope=window{} samples={} coher={} race={} blit={} clean={} slow={} maxus={} wit_us={} frame_us={} -> {}",
            p.id,
            PAYGO_ROLLUP_NOTE,
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
        ); seam_census_emit(p.id); // WCGSEAM-CENSUS — ⚠ SAME-LINE fold, line-NEUTRAL (panic `Location`s below must not renumber). Emitted AFTER the rollup print, so nothing timed follows and no measured bracket widens; compositor context, never print context. Why the census needs a line of its own: the WCGSEAM-CENSUS block at this file's tail.
    }
}

// =================================================================================================
// WINID2 (SO1(b) / A29) — **`SEAM_WIN` is the SIXTH id holder, and it is now registered.**
// =================================================================================================
//
// rmbp 13 (2026-09-06) swept the tree for WINID's SHAPE — a `static AtomicU32` outside `wm` that
// caches a `WinId` across a close — and found one the WINID block's table of five had missed:
// [`SEAM_WIN`], declared beside [`SEAM_WRITES`] in this file. It is stored in exactly ONE place
// ([`seam_glyph_note`]), read in exactly TWO (both in [`end`]), and CLEARED ON NO PATH AT ALL —
// `display_tegra::ORINWM1_WIN`'s exact defect, in a file `video/mod.rs:73` declares unconditionally.
//
// WHY IT MATTERS, stated as the wrong behaviour rather than as a category. Both readers are
// `verdict != "CLEAN" && p.id != 0 && p.id == SEAM_WIN.load(Relaxed)` — the test "is the window this
// pass just convicted the one fbcon charged as the routed console". The FIRST of them is gated
// `#[cfg(not(all(target_arch = "x86_64", feature = "wcg-paygo")))]`, so the aarch64 side compiles
// it, and it is the WCGSEAM-HB REFUND GATE: on a match it may refund the sample's budget spend
// (`TAKEN[w].fetch_sub`) and bump `W_REARM[w]` toward [`REARM_MAX`]. So a recycled id lets ONE
// window's non-CLEAN verdict steer ANOTHER window's seam re-arm — the console closes, `wm` re-issues
// its slot (render7: the console's win 1 came back as quarry's win 1), and the new tenant's
// convictions are then refunded against a bound the console's boot-seam writer earned. The SECOND
// reader only prints, but it prints `[wcgseam] win=N … routed=…` attributing a CONSOLE census to a
// window that is not the console, which is the instrument lying on the wire.
//
// WHAT THE TWO COMPARISONS DO AFTER A CLOSE, now that the registry clears the cell — READ, not
// assumed. `wm::close` stores [`crate::video::wm::WIN_NONE`] into it, and `WIN_NONE` is `0`
// (`wm.rs:183`), which is the value `SEAM_WIN` was BORN with (`AtomicU32::new(0)`). Both call sites
// already lead with `p.id != 0`, and `p.id` is a live row's id, never 0 — so with the cell at 0 the
// equality can hold for no reachable `p.id` and the whole conjunct is INERT. Nothing becomes
// reachable that was not, and nothing that was reachable is lost: the refund gate and the
// `[wcgseam]` print simply stop firing until fbcon charges a routed glyph write again, which
// re-stores a LIVE id. That is precisely the pre-route behaviour this instrument already ships —
// the header note's own x86-QEMU reading, "no takeover, no routed console, `SEAM_WIN` never
// written" — so a close returns the gate to a state the corpus has already characterised.
//
// WHERE THE REGISTRATION IS SITED, AND WHY NOT ON THE STORE LINE. The other five holders register on
// their own `WIN.store(id, …)` line because that line runs ONCE PER WINDOW. `SEAM_WIN`'s does not:
// it is inside [`seam_glyph_note`], which fbcon calls once per GLYPH from its two paint paths, in
// PRINT CONTEXT, under a contract this file states in that function's own doc — "Three relaxed
// atomics and nothing else — called from print context, so it must never take a lock, allocate, or
// print." [`crate::video::wm::winid_register_holder`] takes the `HOLDERS` mutex and, on a full
// registry, prints; folding it onto that line would break the contract on every glyph and put a
// `serial_println!` back into the middle of a `serial_println!`. So the registration is sited on
// [`begin`] instead, and the siting is a DOMINANCE argument, not a convenience: `end` adjudicates
// only a `Probe` that `begin` handed out, so no read of `SEAM_WIN` is reachable without a prior
// `begin`; `begin` is off the print path; and the fold is ahead of the first timed span in the
// function, so no measured bracket widens by one cycle. The boot-once latch below keeps the mutex
// to exactly ONE acquisition per boot rather than leaning on the registry's pointer scan.
//
// THE GATE is `all(witness, any(all(x86_64, wc), all(aarch64, desktop_firmware)))` — TIGHTER than
// the WINID block's, and it is the exact predicate carried by the two lines that WRITE the cell
// (`fbcon.rs:455`, the locked classic path, and `fbcon.rs:1480`, the unlocked split path).
// Co-extensive by construction: on an image where those two charge sites are compiled out
// `SEAM_WIN` can never hold anything but `WIN_NONE`, so there is nothing for a close to clear and
// registering it would only spend one of `WINID_HOLDER_MAX`'s twelve slots (eight until SO10 raised it, orin 17). The metal image is built
// WITHOUT `witness`, so this costs the flown artifact nothing — measured, not assumed:
// `./arroyo kernel8` is `8ff7c1d1f4e8938d…` before and after this change.
//
// LINE-NEUTRAL: this block is a TAIL APPEND and its one call site is a same-line fold onto a
// statement that already existed. No `panic::Location` in this file moves — which matters more here
// than in most, because `wcg.rs` is compiled unconditionally on every image the tree builds.

/// WINID2 — has [`SEAM_WIN`] been handed to `wm`'s holder registry yet? A boot-once latch, so
/// [`begin`] pays one atomic load per pass and the registry mutex is acquired exactly once.
#[cfg(all(
    feature = "witness",
    any(
        all(target_arch = "x86_64", feature = "wc"),
        all(target_arch = "aarch64", feature = "desktop_firmware")
    )
))]
static SEAM_REGISTERED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// WINID2 — register [`SEAM_WIN`] with `wm`'s holder registry, once per boot.
///
/// The `swap` publishes the latch BEFORE the registry call, and that ordering is load-bearing rather
/// than stylistic: [`crate::video::wm::winid_register_holder`]'s registry-full arm prints, so a
/// future caller on a paint path could re-enter this function through fbcon — the latch is already
/// `true` by then and the second entry returns without touching the mutex.
///
/// LOCKFIX: `HOLDERS` is `wm`'s leaf mutex (taken by nothing else, held across nothing), so
/// acquiring it here cannot participate in a cycle with `TABLE`, `WRITER` or `FBCON`.
#[cfg(all(
    feature = "witness",
    any(
        all(target_arch = "x86_64", feature = "wc"),
        all(target_arch = "aarch64", feature = "desktop_firmware")
    )
))]
fn seam_register_once() {
    if SEAM_REGISTERED.load(Ordering::Relaxed) {
        return;
    }
    if SEAM_REGISTERED.swap(true, Ordering::AcqRel) {
        return;
    }
    crate::video::wm::winid_register_holder(&SEAM_WIN, "wcgseam");
}

/// WINID2 — the erasing twin. Without the two `witness` charge sites [`SEAM_WIN`] is never written,
/// so there is no holder to register, and [`begin`]'s fold costs such an image not one byte.
#[cfg(not(all(
    feature = "witness",
    any(
        all(target_arch = "x86_64", feature = "wc"),
        all(target_arch = "aarch64", feature = "desktop_firmware")
    )
)))]
#[inline(always)]
fn seam_register_once() {}

// =================================================================================================
// WCGSEAM-CENSUS — the seam census gets a line that is NOT gated on a conviction
// =================================================================================================
//
// THE DEFECT, measured on the Orin's `render8` flight (2026-09-06, card
// `render8-20260906T1532Z-c24d951`, 16 knobs incl. `UNAOS_WITNESS=1 UNAOS_DESKCASCADE=1`).
//
// Both `[wcgseam]` prints in [`end`] lead with `verdict != "CLEAN"`. render8 produced 25 `[wc-g]`
// samples — 24 `-> CLEAN`, 1 `-> CLEAN+SLOW` — and five rollups, every one `coher=0 race=0 blit=0`.
// Zero convictions, therefore zero `[wcgseam]` lines, therefore a wire on which the seam census is
// INVISIBLE. The flight's own scorer read that silence as a fact about the census —
// "`A29 wcgseam: ABSENT — the seam never ran this boot`" — which the wire does not support: an
// all-CLEAN boot suppresses the print whether the census ran, never ran, or was never compiled.
// That is the absence-is-only-evidence law: `glyphs=0` on a build whose charge sites are `#[cfg]`'d
// out means "never compiled", not "no writer", and today's wire cannot tell the two apart at all.
//
// It is NOT the gate the brief for this work assumed. That brief (orin 6, 2026-08-25) recorded the
// charge sites as x86-only — `fbcon.rs` locked path `any(all(x86_64, wc), all(aarch64, pidesk))`,
// unlocked split path `all(witness, x86_64, wc)` — and concluded `[wcgseam]` "CANNOT print on a
// tegra/Orin build". Both halves have since moved: `a39aff8b` renamed the feature `pidesk` ->
// `desktop_firmware`, and PARFB widened the unlocked split site to the same predicate as its locked
// sibling. Today BOTH sites (`fbcon.rs:455`, `fbcon.rs:1480`) read
// `all(witness, any(all(x86_64, wc), all(aarch64, desktop_firmware)))`, and
// `deskcascade = ["desktop_firmware", …]`, so render8's own image COMPILED the census. The census
// was armed on that flight and the wire said nothing either way.
//
// THE REMEDY IS A DENOMINATOR, NOT A VERDICT. One line beside each `[wc-g] rollup`, carrying the
// three facts a conviction is read against, and gated on NOTHING:
//
//   * `armed=` — were the charge sites COMPILED? A `#[cfg]`-derived constant under the charge
//     sites' own predicate, and the single fact no counter can ever report. `witness` alone is
//     necessary but not sufficient: an aarch64 witness build without `desktop_firmware` (the QEMU
//     virt ramfb witness) runs [`end`], emits rollups, and has no charge site anywhere.
//   * `glyphs=`/`locked=` — [`SEAM_WRITES`] and its FBCON-locked share. `locked=` is what CHOOSES
//     between the two remedies §6.14 named: FBCON-locking the console blit only serialises writers
//     that take the lock, so a census that is all-unlocked rules that remedy out.
//   * `routed=` — [`super::fbcon::console_is_routed`], which splits QUIET's two causes (no route
//     for the glyphs to land in, versus a route with no glyph on it).
//
// The terminal is the three-way split the wire has been missing: `UNARMED` (not compiled, so
// `glyphs=` carries no information), `QUIET` (compiled, never charged), `CHARGED` (compiled and
// charged — after which a boot with no per-conviction `[wcgseam]` line means "no conviction", which
// is real information rather than silence).
//
// WHAT IT COSTS AND WHY IT IS PAID HERE. One line per `[wc-g] rollup`, 1:1 with a line the wire
// already carries, folded onto the rollup print's own closing line so no panic `Location` in this
// file renumbers. It is emitted after that print, so no timed span contains it, and from compositor
// context — never print context, whose serial write would recurse into `_print`.
//
// THE TAG IS DELIBERATE, AND IT IS THE EXISTING ONE. `[wcgseam]` is matched by no FORBID in
// `x86-witness.spec` or `pi4-regression.spec` (both files' `\[wc-g\] .*-> COHER` / `RACE` / `BLIT`
// are untouched by this and stay load-bearing), and none may ever be written for it. `census` is
// the second word so a reader — and any future pattern — can separate the denominator line from the
// per-conviction lines without a new tag to keep clear of the FORBIDs.

/// WCGSEAM-CENSUS — were the two `fbcon` charge sites compiled into THIS image? The predicate is
/// copied verbatim from `video/fbcon.rs:455`/`:1480`; if either moves, this constant is the thing
/// that must move with it, because it is what makes `glyphs=0` readable.
#[cfg(all(
    feature = "witness",
    any(
        all(target_arch = "x86_64", feature = "wc"),
        all(target_arch = "aarch64", feature = "desktop_firmware")
    )
))]
const SEAM_ARMED: bool = true;

/// WCGSEAM-CENSUS — the erasing twin. `false` here is the whole point: it is what lets a reader
/// know that this image's `glyphs=0` is a compile-time fact and not an observation about writers.
#[cfg(not(all(
    feature = "witness",
    any(
        all(target_arch = "x86_64", feature = "wc"),
        all(target_arch = "aarch64", feature = "desktop_firmware")
    )
)))]
const SEAM_ARMED: bool = false;

/// WCGSEAM-CENSUS — print the seam census beside a `[wc-g] rollup`, unconditionally.
///
/// `id` is the ROLLUP's window, so the line pairs with the rollup it follows; `seam_win=` is the
/// census's own window ([`SEAM_WIN`], the routed console as `fbcon` last charged it) and the two are
/// deliberately separate — a rollup for a window that is not the console is exactly the case the
/// per-conviction prints decline, and the reader must be able to see that.
///
/// Reads four relaxed atomics and one `CONSOLE_WIN` load, then prints. No lock, no allocation, and
/// no counter moves — a census that changed what it measures would be a second instrument.
fn seam_census_emit(id: u32) {
    let glyphs = SEAM_WRITES.load(Ordering::Relaxed);
    #[cfg(any(
        all(target_arch = "x86_64", feature = "wc"),
        all(target_arch = "aarch64", feature = "desktop_firmware")
    ))]
    let routed = if super::fbcon::console_is_routed() { "yes" } else { "no" };
    #[cfg(not(any(
        all(target_arch = "x86_64", feature = "wc"),
        all(target_arch = "aarch64", feature = "desktop_firmware")
    )))]
    let routed = "?";
    serial_println!(
        "[wcgseam] census win={} armed={} glyphs={} locked={} seam_win={} routed={} -> {}",
        id,
        if SEAM_ARMED { "yes" } else { "no" },
        glyphs,
        SEAM_LOCKED.load(Ordering::Relaxed),
        SEAM_WIN.load(Ordering::Relaxed),
        routed,
        if !SEAM_ARMED {
            "UNARMED"
        } else if glyphs > 0 {
            "CHARGED"
        } else {
            "QUIET"
        }
    );
}
