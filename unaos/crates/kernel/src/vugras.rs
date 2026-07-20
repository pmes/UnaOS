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

//! VUGRAS — the vug/idle RAS **localizer** (knob `UNAOS_VUGRAS=1`, arm at build time).
//!
//! Context (settled by the VUG-RAS-2 investigation): on the Jetson Orin Nano a RAS Uncorrectable
//! SNOC+ACI **FillWrite** fires on a cache-line **writeback**, so the bad store happens *frames
//! before* the fault is reported — the churn that forces evictions is incidental. The fault ADDR is
//! record-format (XCARVE erratum, commit 3f76b326: bit-63 is a record bit, strip it), and the decoded
//! address lands near DRAM top (heap ~0x2683ca000, and boot-14 decoded to 0x27724f340 — ABOVE heap
//! hi and below DRAM top 0x2_8000_0000).
//!
//! This diagnostic does two things, **both instrument-only and off unless the knob is set** (the
//! default-quiet law — no VUGRAS lines with the knob off):
//!
//! 1. **Force writeback continuously.** A `DC CIVAC` (clean+invalidate to PoC) sweep over the RAM the
//!    system can dirty, driven from every path where the fault has been seen: the vug frame loop
//!    (per frame) and the JD2 console-idle pump (periodic, ~250 ms). Cleaning already-clean lines is
//!    harmless; the point is that a dirty line is written back *now*, so the RAS fires within a frame
//!    (or a period) of the store instead of much later. The sweep alternates two spans so no single
//!    invocation is unbounded: **A = [heap_lo, heap_hi)** and **B = [heap_hi, carveout-clipped top)**
//!    (B only on tegra builds — on the `virt` QEMU gate B is empty so the sweep never touches unmapped
//!    RAM). Span B is **carveout-bounded** (VUG-RAS-ANALYZE): it is clipped to the first firewall
//!    carveout above the heap (`mmu_tegra::VUGRAS_ABOVE_HEAP_TOP`), because DC-cleaning a SNOC-protected
//!    line is *itself* the FillWrite RAS — an unbounded `[heap_hi, DRAM_top)` sweep would self-inflict
//!    the very fault the localizer exists to attribute to a real writer.
//!
//! 2. **Name the candidates.** A one-shot boot witness (post-heap-init) dumps every PA the fault could
//!    decode to — heap span, framebuffer/scanout, Screen back buffer, cursor save-under stash, and the
//!    xHCI rings/contexts/buffers (with the enumerating port flagged) — so a decoded fault ADDR can be
//!    matched against a table from the serial capture alone. See the post-mortem decode procedure in
//!    `docs/dev/OS/01_BOOT_HAL/arch_arm64.md` §JETSON-RAS.

use core::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "tegra")]
use core::sync::atomic::AtomicUsize;

/// Armed at build time by the `vugras` cargo feature (`UNAOS_VUGRAS=1` in `arroyo`). Default OFF: every
/// entry point below early-returns, so an unarmed build is byte-behaviour-identical (default-quiet law).
/// A cargo feature (not a bare `option_env!`) so toggling the knob forces a clean rebuild and the OFF
/// build genuinely drops the code, mirroring the `smpprobe` pattern.
pub const ARMED: bool = cfg!(feature = "vugras");

/// Tegra234/Orin DRAM window top (8 GiB). The above-heap span B's default cap on tegra — the decoded
/// fault ADDRs sit in [heap_hi, DRAM_top) — but the span is further clipped BELOW the first firewall
/// carveout above the heap (see `spans`). On non-tegra aarch64 (the `virt` gate) this window does not
/// exist, so span B is empty there (never `DC CIVAC` an unmapped VA).
pub(crate) const TEGRA_DRAM_TOP: usize = 0x2_8000_0000;

/// VUG-RAS-ANALYZE — exclusive top bound for the localizer's **above-heap** sweep (span B), published
/// once by `mmu_tegra::select_heap_region` from the SAME carveout set that seats the heap. Span B must
/// never `DC CIVAC` a firewall carveout — cleaning a SNOC-protected line is *itself* the FillWrite RAS
/// (dirty or not) — so it is clipped to `[heap_hi, first-carveout-above-heap)` (capped at DRAM top).
/// `0` = unpublished ⇒ the sweep treats span B as empty (fail-safe: never clean unproven RAM). Homed
/// here (not in `mmu_tegra`) so the arch-neutral sweep reaches it without an arch-glob module path.
#[cfg(feature = "tegra")]
pub(crate) static VUGRAS_ABOVE_HEAP_TOP: AtomicUsize = AtomicUsize::new(0);

/// Emit the per-frame / per-tick `swept` witness every this many invocations, so serial isn't flooded.
const WITNESS_EVERY: u64 = 32;

/// One-shot cost-witness latches (span A, span B) — the sweep cost is measured and printed exactly once
/// per span so the capture records it without per-frame noise.
static COST_A_DONE: AtomicBool = AtomicBool::new(false);
static COST_B_DONE: AtomicBool = AtomicBool::new(false);

/// Clean+invalidate `[lo, hi)` to the Point of Coherency (forces any dirty line to RAM *now*). No-op
/// off aarch64 and for an empty/backwards span.
#[inline]
fn civac(lo: usize, hi: usize) {
    if hi <= lo {
        return;
    }
    #[cfg(target_arch = "aarch64")]
    crate::arch::aarch64::cache::clean_invalidate_range(lo, hi - lo);
    #[cfg(not(target_arch = "aarch64"))]
    let _ = (lo, hi);
}

/// The two sweep spans for this build. A is always the live heap; B is the above-heap span on tegra
/// (where the out-of-heap fault ADDRs live) and empty elsewhere.
///
/// VUG-RAS-ANALYZE (H2): span B is **carveout-clipped** to `[heap_hi, VUGRAS_ABOVE_HEAP_TOP)`, the
/// firewall-clean DRAM above the heap that `mmu_tegra` derives from the same carveout set it seats the
/// heap clear of. A raw `[heap_hi, DRAM_top)` sweep would eventually DC-CIVAC a protected carveout, and
/// cleaning a SNOC-firewalled line IS the FillWrite RAS (dirty or not) — the localizer would then
/// self-inflict a fault indistinguishable from a real writer's writeback. Clipping keeps the sweep on
/// RAM we can legitimately clean; a genuine store into a carveout (the boot-15 `0x26b900000` class) is
/// still surfaced by the span-A clean's incidental cache churn, now UNAMBIGUOUSLY not the sweep's doing.
/// Unpublished (`0`) ⇒ empty span B (fail-safe: never clean unproven RAM).
fn spans() -> ((usize, usize), (usize, usize)) {
    let (lo, hi) = crate::allocator::heap_bounds();
    let a = (lo, hi);
    #[cfg(feature = "tegra")]
    let b = {
        let top = VUGRAS_ABOVE_HEAP_TOP.load(Ordering::Relaxed);
        if top > hi {
            (hi, top)
        } else {
            (hi, hi)
        }
    };
    #[cfg(not(feature = "tegra"))]
    let b = (hi, hi);
    (a, b)
}

/// Sweep one span (selected by `parity`), measuring and witnessing its cost once. `parity` even → span
/// A (heap), odd → span B (above-heap). Returns the bytes swept.
fn sweep_one(kind: &str, parity: u64) -> usize {
    let (a, b) = spans();
    let (label, lo, hi, done) = if parity & 1 == 0 {
        ("heap", a.0, a.1, &COST_A_DONE)
    } else {
        ("above-heap", b.0, b.1, &COST_B_DONE)
    };
    let c0 = crate::arch::now_cycles();
    let m0 = crate::arch::ms();
    civac(lo, hi);
    let cyc = crate::arch::now_cycles().wrapping_sub(c0);
    let dms = crate::arch::ms().wrapping_sub(m0);
    let bytes = hi.saturating_sub(lo);
    if !done.swap(true, Ordering::Relaxed) {
        serial_println!(
            ":: VUGRAS: {} sweep cost — span {} [{:#x},{:#x}) {} MiB in {} cycles (~{} ms) ::",
            kind,
            label,
            lo,
            hi,
            bytes / (1024 * 1024),
            cyc,
            dms
        );
    }
    bytes
}

/// vug frame-loop hook: at the end of each rendered frame, sweep one span (alternating A/B by frame
/// parity so no single frame pays the full cost), and bracket the fatal frame with a `swept` witness
/// every `WITNESS_EVERY` frames. No-op when the knob is off.
#[inline]
pub fn frame_sweep(frame: u64) {
    if !ARMED {
        return;
    }
    sweep_one("frame", frame);
    if frame % WITNESS_EVERY == 0 {
        serial_println!(":: VUGRAS: frame {} swept ::", frame);
    }
}

/// JD2 console-idle hook: called on the ~250 ms cadence from the console pump (the shell-idle path that
/// crashed on boot-14 with no vug run). Sweeps one span per tick, alternating A/B, and witnesses each
/// tick (the cadence is slow enough not to flood). No-op when the knob is off.
#[inline]
pub fn idle_sweep(tick: u64) {
    if !ARMED {
        return;
    }
    sweep_one("idle", tick);
    serial_println!(":: VUGRAS: idle tick {} swept ::", tick);
}

/// The heap/framebuffer half of the candidate table (arch-neutral). Also runs one heap-span sweep so a
/// headless boot (the `virt` GICv3 gate, which never drives vug or JD2) still witnesses the sweep and
/// the cost line. Safe on `virt` (only span A, which is mapped). No-op when the knob is off.
pub fn core_witness() {
    if !ARMED {
        return;
    }
    let (lo, hi) = crate::allocator::heap_bounds();
    serial_println!(
        ":: VUGRAS: heap span [{:#x},{:#x}) {} MiB; tegra DRAM top {:#x} ::",
        lo,
        hi,
        hi.saturating_sub(lo) / (1024 * 1024),
        TEGRA_DRAM_TOP
    );
    // VUG-RAS-ANALYZE: name the carveout-clipped above-heap sweep span (span B) so a capture can see it
    // never reaches a firewall carveout. On virt (no tegra) this compiles out and span B stays empty.
    #[cfg(feature = "tegra")]
    {
        let top = VUGRAS_ABOVE_HEAP_TOP.load(Ordering::Relaxed);
        let b_hi = if top > hi { top } else { hi };
        serial_println!(
            ":: VUGRAS: above-heap sweep span B [{:#x},{:#x}) {} KiB — carveout-clipped (never DC-cleans firewall fabric) ::",
            hi,
            b_hi,
            b_hi.saturating_sub(hi) / 1024
        );
    }
    let fb = *crate::video::WRITER.lock();
    if fb.is_ready() {
        serial_println!(
            ":: VUGRAS: framebuffer/scanout base {:#x} len {:#x} ::",
            fb.base(),
            fb.len()
        );
    }
    let (clo, chi) = crate::pal::cursor::saved_pa();
    serial_println!(":: VUGRAS: cursor save-under stash [{:#x},{:#x}) ::", clo, chi);
    // Exercise the sweep once (span A only — always mapped) so the gate observes it running.
    sweep_one("witness", 0);
}

/// Boot witness (post-heap-init, tegra path): the full candidate table — the arch-neutral core plus the
/// xHCI DMA structures (rings, contexts, buffers, enumerating port). Called once, under the knob.
pub fn boot_witness() {
    if !ARMED {
        return;
    }
    serial_println!(":: VUGRAS: boot witness — RAS candidate PA table (bit-63-stripped decode) ::");
    core_witness();
    crate::drivers::xhci::vugras_dump();
}

/// Witness the Screen back buffer span once, when the console/vug builds it (its PA is only known after
/// the `Screen` is constructed). No-op when the knob is off.
pub fn note_screen(screen: &crate::video::Screen) {
    if !ARMED {
        return;
    }
    let (lo, hi) = screen.back_span();
    serial_println!(":: VUGRAS: Screen back buffer [{:#x},{:#x}) ::", lo, hi);
}
