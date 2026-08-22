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

/// XCARVE-2 — the SNOC RAS FillWrite target: real PA **0x26b900000** (record-format ADDR
/// `0x800000026b900000`, bit-63 stripped). 5.2 MiB above heap top (heap ends 0x26b3ca000), below the
/// framebuffer (0x279e00000) — known carveout-free RAM. An UNIDENTIFIED CPU store dirties this line;
/// the eviction (or a localizer sweep) writeback is only the messenger that raises the RAS. Boot-19's
/// "confirmed fixed" was FALSE (that boot died earlier on the 0x200 NET RAS and never reached the
/// idle/sweep path; boot 20 reached it and 0x26b900000 refired). The event-ring/ERST heap move
/// (425090fd) is a KEEP but was NOT the writer. This arc hunts the writer by store-site bracketing.
#[cfg(feature = "tegra")]
pub(crate) const XCARVE_TARGET_PA: usize = 0x2_6b90_0000;

/// Number of witnessed sub-spans span B is bisected into (spatial bracketing). The sub-span whose sweep
/// fires the RAS pins the dirty line's PA to ~1/N of span B — the last `sub-span k/N` line in the
/// capture before the fault is the window. 12 keeps each sub-span ~a few MiB while staying serial-cheap.
const SUBSPANS: usize = 12;

/// Tripwire footprint: `DC CIVAC` the target line ± this many bytes (a few 64 B neighbor lines), so a
/// store landing one or two lines off the nominal PA is still forced back promptly at every bracket.
#[cfg(feature = "tegra")]
const TRIPWIRE_HALF: usize = 192;

/// One-shot cost-witness latches (span A, span B) — the sweep cost is measured and printed exactly once
/// per span so the capture records it without per-frame noise.
static COST_A_DONE: AtomicBool = AtomicBool::new(false);
static COST_B_DONE: AtomicBool = AtomicBool::new(false);

/// XCARVE-3 excluded-carveout hole `(base, size, source)` from `mmu_tegra`. Guarded on `target_arch`
/// (the `tegra` feature is also enabled on the x86 syntax-check target, where `arch::aarch64` does not
/// exist), so off-aarch64 it reports "no hole" and the localizer keeps its pre-XCARVE-3 behavior.
#[cfg(feature = "tegra")]
#[inline]
fn hole_bounds() -> (u64, u64, u8) {
    #[cfg(target_arch = "aarch64")]
    {
        crate::arch::aarch64::mmu_tegra::carveout_hole()
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        (0, 0, 0)
    }
}

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

/// XCARVE-2 spatial bisection: split `[lo,hi)` into `SUBSPANS` witnessed sub-spans, `DC CIVAC` each in
/// turn and emit a `sub-span k/N` witness for it. If the unidentified store has already dirtied a line
/// in one sub-span, the RAS FillWrite fires on THAT sub-span's clean — so the LAST `sub-span k/N` line
/// in the capture pins the dirty line to `[lo + k*step, lo + (k+1)*step)`, ~1/N of the span. Cheap
/// enough (one linear pass, same total work as a plain sweep) to run at every bracket point. No-op for
/// an empty/backwards span (with a witness so the capture records the span was empty, not skipped).
fn bisect(label: &str, lo: usize, hi: usize) {
    if hi <= lo {
        serial_println!(":: VUGRAS: bisect {} — empty span [{:#x},{:#x}), nothing to sweep ::", label, lo, hi);
        return;
    }
    let span = hi - lo;
    let step = span.div_ceil(SUBSPANS);
    let mut k = 0usize;
    let mut base = lo;
    while base < hi {
        let end = core::cmp::min(base + step, hi);
        civac(base, end);
        serial_println!(
            ":: VUGRAS: sub-span {}/{} [{:#x},{:#x}) swept ({}) ::",
            k, SUBSPANS, base, end, label
        );
        base = end;
        k += 1;
    }
}

/// XCARVE-2/3 single-line tripwire (tegra). Boot-21 reframed XCARVE: `0x26b900000` is firmware-protected
/// carveout DRAM with NO software writer — the RAS fired from *this tripwire's own* `DC CIVAC` at
/// post-mmu, because the fabric rejects ANY cache traffic to the window. XCARVE-3's mapping fix therefore
/// UNMAPS the window (`mmu_tegra` L2 hole), and a `DC CIVAC` to an unmapped VA would now translation-fault.
/// So the tripwire no longer touches the target: when the target line falls inside the excluded hole
/// (always, on a tegra build post-`init`) it witnesses the exclusion and issues NO CMO. It only cleans the
/// line on the defensive path where no hole was resolved (`hole_size == 0`), preserving the old bracket for
/// a build where the mapping fix did not seat. No-op when the knob is off.
#[cfg(feature = "tegra")]
pub fn tripwire(site: &str) {
    if !ARMED {
        return;
    }
    let (hb, hs, _) = hole_bounds();
    let excluded = hs != 0 && XCARVE_TARGET_PA as u64 >= hb && (XCARVE_TARGET_PA as u64) < hb + hs;
    if excluded {
        serial_println!(
            ":: VUGRAS: tripwire @ {} — line {:#x} is inside the XCARVE-3 excluded carveout [{:#x},{:#x}); UNMAPPED, NO CMO issued (fabric-protected, no software writer) ::",
            site, XCARVE_TARGET_PA, hb, hb + hs
        );
        return;
    }
    let lo = XCARVE_TARGET_PA.saturating_sub(TRIPWIRE_HALF);
    let hi = XCARVE_TARGET_PA + TRIPWIRE_HALF;
    civac(lo, hi);
    serial_println!(
        ":: VUGRAS: tripwire @ {} — line {:#x} (+/-{} B) swept (no hole resolved) ::",
        site, XCARVE_TARGET_PA, TRIPWIRE_HALF
    );
}

/// XCARVE-2 temporal bracket (tegra): a named boot-phase boundary. Emits a phase witness, fires the
/// single-line tripwire, then bisects span B once. The interval between the LAST phase whose tripwire +
/// bisect swept clean and the phase whose sweep fires the RAS names the code region that made the store.
/// Called at the named boot boundaries (post-MMU, post-heap-init, post-PCIe, post-NET, post-xHCI-attach,
/// shell-entry) from `tegra_early_stop` / `jd2_console_pump`. No-op when the knob is off.
#[cfg(feature = "tegra")]
pub fn phase(name: &str) {
    if !ARMED {
        return;
    }
    serial_println!(":: VUGRAS: === phase {} ===", name);
    tripwire(name);
    let (_, b) = spans();
    bisect("span-B", b.0, b.1);
}

/// XCARVE-2 identity witness (tegra, one-shot): does anything the kernel knows about map/own the target
/// line? For each known region emit whether `XCARVE_TARGET_PA` falls inside it; if NOTHING contains it,
/// say so explicitly — the store is then through a wild/stale pointer, and the temporal bracket (which
/// code region ran between the last clean tripwire and the firing one) is the evidence that matters, not
/// an owning allocation. Same candidate region set `core_witness` prints. No-op when the knob is off.
#[cfg(feature = "tegra")]
pub fn pa_identity_witness() {
    if !ARMED {
        return;
    }
    let t = XCARVE_TARGET_PA;
    serial_println!(
        ":: VUGRAS: XCARVE identity — target line {:#x} (record ADDR {:#x}) ::",
        t,
        0x8000_0000_0000_0000u64 | t as u64
    );
    // XCARVE-6 one-shot excluded-SET witness: list EVERY protected window the MMU unmapped (the two QUIRKs
    // 0x26b9/0xbe + every DTB /reserved-memory carveout inside a RAM GiB), so a capture accounts for the
    // whole exclusion set (XCARVE-5 no-silent-drop law), not just the 0x26b9 primary.
    #[cfg(target_arch = "aarch64")]
    {
        let mut set = [(0u64, 0u64, 0u8); crate::arch::aarch64::mmu_tegra::MAX_HOLES];
        let n = crate::arch::aarch64::mmu_tegra::carveout_holes(&mut set);
        serial_println!(":: VUGRAS: XCARVE-6 excluded set — {} protected window(s) unmapped ::", n);
        for i in 0..n {
            let (b, s, src) = set[i];
            let ssrc = match src {
                1 => "DTB /reserved-memory",
                2 => "QUIRK (DTB silent)",
                _ => "unknown",
            };
            serial_println!(
                ":: VUGRAS:   window[{}] [{:#x},{:#x}) {} KiB @ GiB {} (source: {}) ::",
                i, b, b + s, s / 1024, b >> 30, ssrc
            );
        }
    }
    // XCARVE-3 one-shot excluded-window witness: the boot-21 verdict is that the target is fabric-protected
    // carveout DRAM (no software writer). Report the PRIMARY window (0x26b9) and its source (DTB vs QUIRK).
    let (hb, hs, src) = hole_bounds();
    if hs != 0 {
        let ssrc = match src {
            1 => "DTB /reserved-memory",
            2 => "QUIRK (DTB silent)",
            _ => "unknown",
        };
        serial_println!(
            ":: VUGRAS: XCARVE-3 excluded window — [{:#x},{:#x}) {} KiB UNMAPPED (source: {}) ::",
            hb,
            hb + hs,
            hs / 1024,
            ssrc
        );
        if t as u64 >= hb && (t as u64) < hb + hs {
            serial_println!(
                ":: VUGRAS: XCARVE identity — {:#x} IS the XCARVE-3 excluded carveout (unmapped, no cacheable translation) — no software writer; RAS retired by exclusion ::",
                t
            );
            serial_println!(":: VUGRAS: XCARVE RO-map hard tripwire — MOOT (window unmapped by XCARVE-3; no page to protect) ::");
            return;
        }
    }
    let mut hit = false;
    let (hlo, hhi) = crate::allocator::heap_bounds();
    if t >= hlo && t < hhi {
        serial_println!(":: VUGRAS: XCARVE identity — {:#x} IS INSIDE heap [{:#x},{:#x}) ::", t, hlo, hhi);
        hit = true;
    }
    let top = VUGRAS_ABOVE_HEAP_TOP.load(Ordering::Relaxed);
    let b_hi = if top > hhi { top } else { hhi };
    if t >= hhi && t < b_hi {
        serial_println!(
            ":: VUGRAS: XCARVE identity — {:#x} IS INSIDE span-B (carveout-free above-heap) [{:#x},{:#x}) ::",
            t, hhi, b_hi
        );
        hit = true;
    }
    let fb = *crate::video::WRITER.lock();
    if fb.is_ready() {
        let (flo, fhi) = (fb.base(), fb.base() + fb.len());
        if t >= flo && t < fhi {
            serial_println!(":: VUGRAS: XCARVE identity — {:#x} IS INSIDE framebuffer [{:#x},{:#x}) ::", t, flo, fhi);
            hit = true;
        }
    }
    let (clo, chi) = crate::pal::cursor::saved_pa();
    if clo != chi && t >= clo && t < chi {
        serial_println!(":: VUGRAS: XCARVE identity — {:#x} IS INSIDE cursor save-under [{:#x},{:#x}) ::", t, clo, chi);
        hit = true;
    }
    if !hit {
        serial_println!(
            ":: VUGRAS: XCARVE identity — {:#x} is NOT inside any known kernel region (heap/span-B/fb/cursor); WILD/STALE-POINTER store hypothesis — the temporal bracket names the writer ::",
            t
        );
    }
    // RO-map hard tripwire decision (brief step 4): DISARMED this arc. Read-only-mapping the containing
    // page so the store faults with ELR = the store site would require a page-table edit in mmu_tegra and
    // CANNOT be validated on the QEMU gate (the `virt` model has no such above-heap RAM region and never
    // exercises the tegra identity map), so arming it blind risks perturbing a legitimate mapping on the
    // one platform where it runs — exactly what the brief forbids. The DC-CIVAC tripwire + spatial
    // bisection + temporal phase brackets localize the store without touching page permissions; a future
    // arc can arm the RO-map once this identity witness has established (on metal) whether the page is
    // ever legitimately written. Witnessed either way, per the brief.
    serial_println!(":: VUGRAS: XCARVE RO-map hard tripwire — DISARMED (unvalidatable on QEMU gate; DC-CIVAC tripwire + bisect + phase brackets localize without touching page perms) ::");
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
    // XCARVE-2 spatial bracket: bisect span A (the live heap — always mapped, so this runs on the virt
    // GICv3 gate too) into witnessed sub-spans. On tegra `boot_witness` additionally bisects span B, the
    // above-heap span that actually covers the 0x26b900000 XCARVE target.
    bisect("span-A (heap)", lo, hi);
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
    // XCARVE-3: report the excluded carveout window (the boot-21 verdict — 0x26b900000 is fabric-protected
    // DRAM, no software writer), then bisect span B. Span B is now clipped BELOW the excluded hole by
    // `VUGRAS_ABOVE_HEAP_TOP`, so the bisect provably never DC-CIVACs the protected window.
    #[cfg(feature = "tegra")]
    {
        pa_identity_witness();
        let (_, b) = spans();
        bisect("span-B (above-heap, clipped below the XCARVE-3 excluded carveout)", b.0, b.1);
    }
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
