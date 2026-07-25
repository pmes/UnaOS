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
//! Two EL0 apps with unrelated paint loops (the uvug crystal's 300-frame renderer and `kvug.elf`'s
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
