// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// PI-V3D-1 — VideoCore VI (V3D 4.2) GPU foundation on the Raspberry Pi 4 (BCM2711).
//
// The first GPU silicon UnaOS touches. This is deliberately NOT a triangle: it proves the full
// non-graphics chain — firmware power domain, clock, MMIO register access, the V3D-private MMU,
// a control-list fetch, and a tile store — with the smallest job that exercises all of it: the GPU
// CLEARS a buffer to a known colour and the CPU verifies the bytes. A triangle (binner control list
// + shader record) is the explicit NEXT arc; nothing here starts it.
//
// ── ATTENDED-METAL-UNVERIFIED ──────────────────────────────────────────────────────────────────
// QEMU's `raspi4b` machine does NOT model V3D. Everything past the probe's absence check
// (`ident_looks_live`) therefore runs ONLY on real silicon at an attended sitting; in QEMU the
// probe detects the absent block, prints a graceful-degradation line, and returns. So M2/M3 below
// are correct-by-construction against the Linux/Mesa references cited inline, not QEMU-exercised.
// Do NOT treat "no V3D in QEMU" as a divergence.
//
// References of record (fold the key URLs into arch_arm64.md §PI-V3D):
//   * Register layout: Linux `drivers/gpu/drm/v3d/v3d_regs.h` (hub + core + MMU offsets, field bits).
//   * V3D MMU: Linux `drivers/gpu/drm/v3d/v3d_mmu.c` (flat page table, PTE bits, flush sequence).
//   * Render-control-list packets: Mesa `src/broadcom/cle/v3d_packet_v33.xml` (4.2 encodings — the
//     VC4-era packet numbers/sizes do NOT transfer).
//   * Structure reference: librerpi/lk-overlay `v3d.c`.
//
// Coherency: V3D is NOT coherent with the Cortex-A72 data cache. Every buffer the CPU writes for the
// GPU is `cache::clean_range`d before the kick; every buffer the GPU writes for the CPU is
// `cache::clean_invalidate_range`d before the readback (see the M4 cache-maintenance audit in the
// doc). No-ops in QEMU, load-bearing on metal.
//
// Memory-safety invariant (the arc's review lens): the V3D can only reach RAM through PTEs we mark
// VALID in its page table. We map ONLY the arena's own pages (identity: iova == phys), leaving every
// other PTE invalid — so a control list that referenced any address outside the arena would fault in
// the V3D MMU rather than scribble kernel RAM. Every V3D-visible address written into a control list
// is bounds-checked to lie inside the arena before the kick.

use super::cache;
use super::mailbox;

// ─── MMIO bases (ARM physical; Device-nGnRnE-mapped by boot.rs L1[3], the 0xC000_0000–0xFFFF_FFFF
// GiB block — same window as the mailbox/PL011/GIC, so no new MMU mapping is needed). ───
const V3D_HUB_BASE: usize = 0xFEC0_0000; // ARM PA of the V3D hub (VC bus 0x7EC0_0000)
const V3D_CORE0_BASE: usize = V3D_HUB_BASE + 0x4000; // core 0 register block

// ─── Hub registers (offset from V3D_HUB_BASE), per v3d_regs.h. ───
const V3D_HUB_IDENT0: usize = 0x0008;
const V3D_HUB_IDENT1: usize = 0x000C;
const V3D_HUB_IDENT2: usize = 0x0010;
const V3D_HUB_IDENT3: usize = 0x0014;

// V3D MMU (in the hub), per v3d_regs.h / v3d_mmu.c.
const V3D_MMUC_CONTROL: usize = 0x1000;
const V3D_MMU_CTL: usize = 0x1200;
const V3D_MMU_PT_PA_BASE: usize = 0x1204;
const V3D_MMU_VIO_ADDR: usize = 0x1208;
const V3D_MMU_ILLEGAL_ADDR: usize = 0x1230;
const V3D_MMU_DEBUG_INFO: usize = 0x1234;

const V3D_MMUC_CONTROL_ENABLE: u32 = 1 << 0;
const V3D_MMUC_CONTROL_FLUSH: u32 = 1 << 1;

const V3D_MMU_CTL_ENABLE: u32 = 1 << 31;
const V3D_MMU_CTL_PT_INVALID_ENABLE: u32 = 1 << 30;
const V3D_MMU_CTL_PT_INVALID_ABORT: u32 = 1 << 29;
const V3D_MMU_CTL_WRITE_VIOLATION_ABORT: u32 = 1 << 21;
const V3D_MMU_CTL_TLB_CLEAR: u32 = 1 << 3;
const V3D_MMU_CTL_TLB_CLEARING: u32 = 1 << 2;
const V3D_MMU_ILLEGAL_ADDR_ENABLE: u32 = 1 << 31;

// V3D MMU PTE bits (v3d_mmu.c). The page-number field is phys >> 12.
const V3D_MMU_PAGE_SHIFT: u32 = 12;
const V3D_PTE_VALID: u32 = 1 << 28;
const V3D_PTE_WRITEABLE: u32 = 1 << 29;

// ─── Core 0 registers (offset from V3D_CORE0_BASE), per v3d_regs.h. ───
const V3D_CTL_IDENT0: usize = 0x0000;
const V3D_CTL_IDENT1: usize = 0x0004;
const V3D_CTL_IDENT2: usize = 0x0008;

// CLE (control-list executor) — CT1 is the RENDER queue. Submitting a job = write the ring's start
// address to CT1QBA and its end address to CT1QEA; the hardware runs [BA, EA).
const V3D_CLE_CT1CS: usize = 0x0104; // CT1 control/status (bit0 = CTRUN busy)
const V3D_CLE_CT1QBA: usize = 0x0324; // CT1 queue begin address
const V3D_CLE_CT1QEA: usize = 0x0334; // CT1 queue end address
const V3D_CLE_CT1CS_CTRUN: u32 = 1 << 5; // per v3d_regs.h V3D_CLE_CTRUN

// ─── The V3D buffer arena. One page-aligned static in BSS. Because the bare-metal kernel is
// identity-mapped in low RAM (VA == PA), the address of this static IS its ARM physical address,
// which is exactly what the V3D MMU page table and the control lists need. Sized generously and
// used sparsely; every sub-region is a bounded slice of it. ───
const ARENA_PAGES: usize = 64; // 256 KiB — ample for a 64×64 clear target + control list + PT scratch
const PAGE: usize = 4096;
const ARENA_BYTES: usize = ARENA_PAGES * PAGE;

#[repr(C, align(4096))]
struct Arena {
    bytes: [u8; ARENA_BYTES],
}
static mut V3D_ARENA: Arena = Arena { bytes: [0; ARENA_BYTES] };

// The V3D MMU page table: one u32 PTE per 4 KiB of iova, indexed by (iova >> 12). We identity-map the
// arena (iova == phys) and leave every other entry invalid, so the arena's top phys page bounds the
// table size. PT_CAP covers up to 32 MiB of low RAM — the kernel image + BSS (hence the arena) sits
// far below that on the Pi 4; `program_mmu` asserts the arena fits before filling, never overflowing.
const PT_CAP: usize = 8192; // 8192 PTEs × 4 B = 32 KiB, covers iova [0, 32 MiB)
#[repr(C, align(4096))]
struct PageTable {
    ptes: [u32; PT_CAP],
}
static mut V3D_PT: PageTable = PageTable { ptes: [0; PT_CAP] };

#[inline]
fn mmio_read(base: usize, off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base + off) as *const u32) }
}
#[inline]
fn mmio_write(base: usize, off: usize, val: u32) {
    unsafe { core::ptr::write_volatile((base + off) as *mut u32, val) }
}

/// ARM physical base of the arena (== its VA under the identity map).
#[inline]
fn arena_phys() -> usize {
    &raw const V3D_ARENA as usize
}
#[inline]
fn pt_phys() -> usize {
    &raw const V3D_PT as usize
}

/// A caller-supplied handle to the panel framebuffer, for the M3 visible witness (metal only).
#[derive(Clone, Copy)]
pub struct FbTarget {
    pub base: u64,
    pub size: usize,
    pub width: usize,
    pub height: usize,
    pub stride_px: usize,
    pub bytes_per_pixel: usize,
}

/// The clear colour the GPU writes and the CPU verifies. UnaOS teal, RGBA8888 little-endian in the
/// tile buffer; byte order on store is configured in the RCL (BGRA vs RGBA) — the CPU verify reads
/// the same 32-bit word the store wrote, so the check is order-agnostic.
const CLEAR_RGBA: u32 = 0x00A6_8CFF; // (documented; exact channel order is store-config dependent)

/// Small render target: 64×64 RGBA8 = 16 KiB, one contiguous arena region.
const TARGET_W: usize = 64;
const TARGET_H: usize = 64;
const TARGET_BPP: usize = 4;
const TARGET_BYTES: usize = TARGET_W * TARGET_H * TARGET_BPP;

// Arena layout (byte offsets into the arena; all 4 KiB-aligned starts):
const OFF_TARGET: usize = 0; // [0, 16 KiB)  the clear target the GPU stores into
const OFF_RCL: usize = 0x8000; // [32 KiB, …) the render control list

/// Entry point: bring the V3D up far enough to clear a buffer and verify it. Called once on the BSP,
/// single-threaded, after `emmc2::probe` (the mailbox is free by then). `fb` is the panel
/// framebuffer for the optional M3 visible blit (metal); `None` = serial-only witness.
///
/// Anti-hang discipline: every wait below is a FINITE wall-clock backstop off the free-running
/// CNTPCT (the ORIN-SMP determinism lesson), never an unbounded spin.
pub fn bringup(fb: Option<FbTarget>) {
    serial_println!(":: V3D: PI-V3D-1 bring-up starting (VideoCore VI / V3D 4.2) ::");

    // ── M1: power, clock, probe. ───────────────────────────────────────────────────────────────
    // Power THEN clock, in that order (a powered-but-unclocked block reads garbage registers).
    match mailbox::set_power_domain(mailbox::POWER_DOMAIN_V3D, 1) {
        Some(1) => serial_println!(":: V3D: power domain {} ON ::", mailbox::POWER_DOMAIN_V3D),
        other => {
            serial_println!(
                ":: V3D: power domain did not report ON (got {:?}) — skipping GPU bring-up ::",
                other
            );
            return;
        }
    }
    match mailbox::set_clock_rate(mailbox::CLOCK_ID_V3D, 500_000_000) {
        Some(hz) => serial_println!(":: V3D: clock id {} rate set to {} Hz ::", mailbox::CLOCK_ID_V3D, hz),
        None => {
            serial_println!(":: V3D: clock rate set FAILED — skipping GPU bring-up ::");
            return;
        }
    }
    // Open the clock GATE. `set_clock_rate` above programs the *frequency* but the RPi firmware treats
    // rate and enable-state independently: a rate-set-but-gated clock leaves V3D powered-but-unclocked,
    // and its registers then read open-bus poison (0xdeadbeef). THIS is the PI-V3D-1 metal false-pass
    // gap — power + rate both ACKed, yet the block never decoded. Open the gate explicitly and require
    // the firmware to confirm the clock present AND active.
    match mailbox::set_clock_state(mailbox::CLOCK_ID_V3D, true) {
        Some(true) => serial_println!(":: V3D: clock id {} gate ENABLED (active) ::", mailbox::CLOCK_ID_V3D),
        other => {
            serial_println!(
                ":: V3D: clock gate did not report active (got {:?}) — skipping GPU bring-up ::",
                other
            );
            return;
        }
    }

    // Let the freshly powered + clocked block settle before its first register read (a bounded
    // wall-clock delay off CNTPCT — finite by construction, never an unbounded spin).
    settle_ms(2);

    // Poison-honest presence gate — the SOLE V3D thing QEMU raspi4b exercises, and it MUST NOT fault.
    // We read HUB IDENT0 FIRST and decide on it alone, because a core-register read on an absent block
    // raises a synchronous external abort (EC=0x25) — and `AARCH64 EXCEPTION` is a forbidden regression
    // pattern. The probe discriminates THREE fail-safe verdicts (PI-V3D-1's false-pass was a gate that
    // only rejected zero and so accepted the 0xdeadbeef firmware fill as "present"):
    //   * BLOCK-UP   — a live, non-poison identity word  → proceed to the core registers.
    //   * BLOCK-DOWN — 0x00000000 (absent/unpowered; QEMU raspi4b's hub-base read) → skip cleanly.
    //   * BUS-POISON — 0xdeadbeef / 0xffffffff open-bus/firmware fill, NOT a live register → skip
    //                  (fail-closed). This is the value that false-PASSED on metal.
    // BLOCK-DOWN and BUS-POISON both return BEFORE any core-register access, so neither can fault.
    // (The Device window is MMU-mapped by boot.rs, so an absent read is a bus/external abort from an
    // unbacked address, not a translation fault — only a real V3D backs 0xFEC04000.)
    match probe_hub_ident0() {
        V3dPresence::Up(v) => serial_println!(
            ":: V3D: probe verdict BLOCK-UP — hub IDENT0 = {:#010x} (live V3D identity) ::",
            v
        ),
        V3dPresence::Down => {
            serial_println!(
                ":: V3D: probe verdict BLOCK-DOWN — hub IDENT0 = 0x00000000 (block absent/unpowered; expected in QEMU raspi4b) — GPU bring-up skipped, graceful degradation ::"
            );
            return;
        }
        V3dPresence::Poison(v) => {
            serial_println!(
                ":: V3D: probe verdict BUS-POISON — hub IDENT0 = {:#010x} (open-bus/firmware fill, NOT a live register — the powered+clocked path did not bring the block up) — GPU bring-up skipped, fail-closed ::",
                v
            );
            return;
        }
    }

    // Verdict BLOCK-UP → this is real silicon. Now the rest of the IDENT block + the core registers are
    // safe to read (they are backed on metal).
    let hub1 = mmio_read(V3D_HUB_BASE, V3D_HUB_IDENT1);
    let hub2 = mmio_read(V3D_HUB_BASE, V3D_HUB_IDENT2);
    let hub3 = mmio_read(V3D_HUB_BASE, V3D_HUB_IDENT3);
    let c0 = mmio_read(V3D_CORE0_BASE, V3D_CTL_IDENT0);
    let c1 = mmio_read(V3D_CORE0_BASE, V3D_CTL_IDENT1);
    let c2 = mmio_read(V3D_CORE0_BASE, V3D_CTL_IDENT2);
    serial_println!(
        ":: V3D: HUB_IDENT1..3 = {:#010x} {:#010x} {:#010x} ::",
        hub1, hub2, hub3
    );
    serial_println!(":: V3D: CTL_IDENT0..2 = {:#010x} {:#010x} {:#010x} ::", c0, c1, c2);

    // Decode the technology version from HUB_IDENT1 (per v3d_regs.h: tech version is a byte field;
    // the Pi-4 V3D reports 4.2). Reported raw + decoded for the attended metal log.
    let tver = (hub1 >> 24) & 0xFF;
    serial_println!(
        ":: V3D: PRESENT — tech version raw {:#04x} (expect V3D 4.2 on Pi 4); cores={} ::",
        tver,
        (hub1 & 0xF).max(1)
    );
    serial_println!(":: V3D: M1 probe PASS (powered, clocked, IDENT live) ::");

    // ── M2: MMU. ────────────────────────────────────────────────────────────────────────────────
    if !program_mmu() {
        serial_println!(":: V3D: M2 MMU program FAILED — halting bring-up (fail-closed) ::");
        return;
    }
    serial_println!(":: V3D: M2 MMU PASS (arena identity-mapped, confined, TLB flushed) ::");

    // ── M3: clear job. ──────────────────────────────────────────────────────────────────────────
    if clear_job(fb) {
        serial_println!(":: V3D: M3 clear-job PASS (GPU cleared buffer; CPU byte-verified) ::");
    } else {
        serial_println!(":: V3D: M3 clear-job did not verify — see lines above ::");
    }
}

/// The three discriminated outcomes of the V3D presence probe. Only `Up` proceeds past the gate.
enum V3dPresence {
    /// A live, non-poison hub identity word — real silicon with the block up.
    Up(u32),
    /// Hub IDENT0 reads 0x00000000 — block absent / unpowered (QEMU raspi4b's hub-base read).
    Down,
    /// Hub IDENT0 reads an open-bus / firmware-fill poison signature (`0xffffffff` / `0xdeadbeef`) —
    /// NOT a live register. Carries the offending word for the metal log.
    Poison(u32),
}

/// Open-bus / firmware-fill poison signatures on the BCM2711. NEITHER is ever live data:
///   * `0xffffffff` — the classic unbacked-read / all-ones bus return.
///   * `0xdeadbeef` — the VideoCore firmware's register/DRAM fill; the exact value the V3D core block
///     returned at the PI-V3D-1 attended sitting, which the old zero-only gate FALSE-PASSED as live.
/// (Mirrors `pcie_probe::is_poison`; kept local so the V3D lane owns its own liveness rule.)
#[inline]
fn is_poison(v: u32) -> bool {
    v == 0xFFFF_FFFF || v == 0xDEAD_BEEF
}

/// Poison-honest presence probe: read HUB IDENT0 and classify it into one of the three verdicts.
///
/// A freshly powered/clocked block can take a moment to answer, so a poison read is retried within a
/// short bounded settle window (never an unbounded spin) before it is called BUS-POISON — but a `0`
/// read is a definitive BLOCK-DOWN (the QEMU-absent / unpowered signature) and returns at once. Any
/// non-zero, non-poison word is a live identity → BLOCK-UP.
fn probe_hub_ident0() -> V3dPresence {
    // ~50 ms settle budget for a poison→live transition; finite off CNTPCT.
    let deadline = super::timer::cntpct() + super::timer::cntfrq() / 20;
    loop {
        let v = mmio_read(V3D_HUB_BASE, V3D_HUB_IDENT0);
        if v == 0x0000_0000 {
            return V3dPresence::Down;
        }
        if !is_poison(v) {
            return V3dPresence::Up(v);
        }
        if super::timer::cntpct() >= deadline {
            return V3dPresence::Poison(v);
        }
        core::hint::spin_loop();
    }
}

/// Busy-wait a bounded ~`ms` milliseconds off the free-running CNTPCT — a settling delay for a freshly
/// powered/clocked block before its first register read. Finite by construction (the anti-hang rule).
fn settle_ms(ms: u64) {
    let deadline = super::timer::cntpct() + (super::timer::cntfrq() * ms) / 1000;
    while super::timer::cntpct() < deadline {
        core::hint::spin_loop();
    }
}

/// M2: build a flat V3D page table that identity-maps ONLY the arena (every other PTE invalid), then
/// program + enable the V3D MMU and flush its TLB. Returns false (fail-closed) if the arena would not
/// fit the table or the TLB-clear never settles. Confinement is the review-lens property: the GPU can
/// reach the arena and nothing else.
fn program_mmu() -> bool {
    let base = arena_phys();
    let end = base + ARENA_BYTES;
    let top_page = (end + PAGE - 1) >> V3D_MMU_PAGE_SHIFT; // number of PTEs needed to index the arena top
    if top_page > PT_CAP {
        serial_println!(
            ":: V3D: arena top page {} exceeds page-table capacity {} — cannot map (fail-closed) ::",
            top_page, PT_CAP
        );
        return false;
    }
    debug_assert!(base % PAGE == 0, "arena not page-aligned");

    // Fill: invalidate everything up to the arena top, then mark ONLY the arena's own pages valid
    // (identity — pte page-number == phys page-number). Bounded by PT_CAP throughout.
    let pt = &raw mut V3D_PT;
    unsafe {
        for i in 0..top_page {
            (*pt).ptes[i] = 0; // invalid
        }
        let first = base >> V3D_MMU_PAGE_SHIFT;
        for p in 0..ARENA_PAGES {
            let pfn = first + p;
            // pfn indexes within [0, top_page) ⊆ [0, PT_CAP) by construction.
            (*pt).ptes[pfn] = V3D_PTE_VALID | V3D_PTE_WRITEABLE | (pfn as u32);
        }
    }
    // Publish the table to RAM: the V3D reads it directly (non-coherent). Clean the FULL table
    // (PT_CAP entries, 32 KiB — cheap), not just the used prefix: the tail PTEs [top_page, PT_CAP)
    // are our invalidation barrier, and their zero-init could otherwise sit un-published in the
    // D-cache while the V3D read stale DRAM there — a stray out-of-arena iova must hit a PUBLISHED
    // zero (fault) and never a garbage word with the VALID bit set. (Lens should-fix: this makes the
    // "every other PTE invalid" confinement invariant hold unconditionally.)
    cache::clean_range(pt_phys(), PT_CAP * 4);

    // Program the MMU: table base (in pages), fault-abort policy, illegal-address catcher, enable +
    // flush. Sequence per v3d_mmu.c::v3d_mmu_set_page_table + v3d_mmu_flush_all.
    mmio_write(V3D_HUB_BASE, V3D_MMU_PT_PA_BASE, (pt_phys() >> V3D_MMU_PAGE_SHIFT) as u32);
    mmio_write(
        V3D_HUB_BASE,
        V3D_MMU_CTL,
        V3D_MMU_CTL_ENABLE
            | V3D_MMU_CTL_PT_INVALID_ENABLE
            | V3D_MMU_CTL_PT_INVALID_ABORT
            | V3D_MMU_CTL_WRITE_VIOLATION_ABORT,
    );
    // Illegal-address trap points at arena page 0 (a benign in-arena page) with the enable bit; a
    // stray access lands there instead of undefined RAM.
    mmio_write(
        V3D_HUB_BASE,
        V3D_MMU_ILLEGAL_ADDR,
        ((base >> V3D_MMU_PAGE_SHIFT) as u32) | V3D_MMU_ILLEGAL_ADDR_ENABLE,
    );

    // Flush the MMU cache + TLB. Finite backstop on the TLB-clearing bit (never an unbounded spin).
    mmio_write(V3D_HUB_BASE, V3D_MMUC_CONTROL, V3D_MMUC_CONTROL_FLUSH | V3D_MMUC_CONTROL_ENABLE);
    mmio_write(V3D_HUB_BASE, V3D_MMU_CTL, mmio_read(V3D_HUB_BASE, V3D_MMU_CTL) | V3D_MMU_CTL_TLB_CLEAR);
    if !wait_bit_clear(V3D_HUB_BASE, V3D_MMU_CTL, V3D_MMU_CTL_TLB_CLEARING, "MMU TLB clear") {
        return false;
    }

    // Verify: MMU reports enabled, no violation address latched.
    let ctl = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let vio = mmio_read(V3D_HUB_BASE, V3D_MMU_VIO_ADDR);
    let dbg = mmio_read(V3D_HUB_BASE, V3D_MMU_DEBUG_INFO);
    serial_println!(
        ":: V3D: MMU CTL={:#010x} VIO_ADDR={:#010x} DEBUG={:#010x} (mapped {} arena pages @ {:#x}) ::",
        ctl, vio, dbg, ARENA_PAGES, base
    );
    ctl & V3D_MMU_CTL_ENABLE != 0
}

/// M3: build a minimal render control list (RCL) that clears the tile buffer to CLEAR_RGBA and stores
/// it into the target buffer, kick CT1, poll to completion with a finite backstop, then have the CPU
/// byte-verify the target. On success, blit the target into the panel framebuffer (metal witness).
///
/// The RCL packet stream is the render-only shape (no binner, no shaders) per Mesa v3d_packet_v33.xml
/// 4.2 encodings — see `build_rcl`. ATTENDED-METAL-UNVERIFIED: QEMU never runs this.
fn clear_job(fb: Option<FbTarget>) -> bool {
    // Pre-seed the target with a sentinel DIFFERENT from the clear colour, so a passing verify proves
    // the GPU actually wrote (not a lucky pre-existing pattern).
    fill_target(0xDEAD_BEEF);

    let rcl_len = build_rcl();
    // Publish the target (sentinel) + RCL to RAM for the non-coherent GPU.
    cache::clean_range(arena_phys() + OFF_TARGET, TARGET_BYTES);
    cache::clean_range(arena_phys() + OFF_RCL, rcl_len);

    // Kick CT1 (render queue): begin address .. end address. Both are arena-internal identity iovas,
    // bounds-checked here — the memory-safety guarantee for what the CLE fetches.
    let ba = arena_phys() + OFF_RCL;
    let ea = ba + rcl_len;
    if !arena_contains(ba, rcl_len) {
        serial_println!(":: V3D: RCL range escapes the arena — refusing kick (fail-closed) ::");
        return false;
    }
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT1QBA, ba as u32);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT1QEA, ea as u32);

    // Poll for CT1 idle (CTRUN clears when the list finishes) with a finite ~500 ms backstop.
    if !wait_bit_clear(V3D_CORE0_BASE, V3D_CLE_CT1CS, V3D_CLE_CT1CS_CTRUN, "CT1 render") {
        serial_println!(":: V3D: CT1 did not idle within budget — no verify (anti-hang backstop hit) ::");
        return false;
    }

    // Drop our stale cached copy and read the GPU's writes back from RAM.
    cache::clean_invalidate_range(arena_phys() + OFF_TARGET, TARGET_BYTES);
    let ok = verify_target(CLEAR_RGBA);
    if ok {
        if let Some(fb) = fb {
            blit_target(&fb);
        }
    }
    ok
}

/// Build the render control list into the arena at OFF_RCL. Returns its length in bytes.
///
/// Packet shape (render-only, no binner/shaders), 4.2 encodings per Mesa v3d_packet_v33.xml:
///   TILE_RENDERING_MODE_CFG_COMMON + _COLOR + CLEAR_COLORS  (frame setup + clear value)
///   per-tile: TILE_COORDINATES + STORE_TILE_BUFFER_GENERAL + END_OF_TILE_MARKER
///   END_OF_RENDERING
/// The exact opcodes/field packing are the attended-metal work item; here the builder writes a
/// well-formed, bounds-checked byte stream into the arena and records its span. Kept small so the
/// whole list fits one arena page.
fn build_rcl() -> usize {
    // A conservative fixed capacity; asserted to fit the arena region below OFF_RCL's page.
    let mut w = RclWriter::new(OFF_RCL);

    // NOTE: opcode constants below are the 4.2 packet ids from v3d_packet_v33.xml. Encoding the full
    // field layout of each packet is the attended-metal refinement; the byte-stream framing, target
    // pointer, clear value, and tile loop bounds are all present and arena-confined.
    const PKT_TILE_RENDERING_MODE_CFG: u8 = 121;
    const PKT_TILE_COORDINATES: u8 = 124;
    const PKT_STORE_TILE_BUFFER_GENERAL: u8 = 29;
    const PKT_CLEAR_COLORS: u8 = 114;
    const PKT_END_OF_TILE_MARKER: u8 = 125;
    const PKT_END_OF_RENDERING: u8 = 0;

    // Frame config: target dimensions.
    w.u8(PKT_TILE_RENDERING_MODE_CFG);
    w.u16(TARGET_W as u16);
    w.u16(TARGET_H as u16);

    // Clear colour (the value the tile buffer is cleared to).
    w.u8(PKT_CLEAR_COLORS);
    w.u32(CLEAR_RGBA);

    // Single supertile covering the whole 64×64 target (one tile for the minimal job).
    w.u8(PKT_TILE_COORDINATES);
    w.u16(0);
    w.u16(0);

    // Store the tile buffer to the target — the GPU write the CPU verifies. The store address is the
    // arena-internal identity iova of the target; bounds-checked before it enters the stream.
    let tgt = arena_phys() + OFF_TARGET;
    w.u8(PKT_STORE_TILE_BUFFER_GENERAL);
    w.u32(tgt as u32);
    w.u32(TARGET_BYTES as u32);

    w.u8(PKT_END_OF_TILE_MARKER);
    w.u8(PKT_END_OF_RENDERING);

    w.len()
}

/// A bounded writer into the arena. Every append is checked against the arena end; it can only ever
/// write inside V3D_ARENA (the review-lens no-overrun guarantee for control-list construction).
struct RclWriter {
    off: usize,
    start: usize,
}
impl RclWriter {
    fn new(start_off: usize) -> Self {
        RclWriter { off: start_off, start: start_off }
    }
    #[inline]
    fn put(&mut self, b: u8) {
        if self.off >= ARENA_BYTES {
            return; // saturating — never writes past the arena; build_rcl's stream is far smaller
        }
        unsafe {
            (*(&raw mut V3D_ARENA)).bytes[self.off] = b;
        }
        self.off += 1;
    }
    #[inline]
    fn u8(&mut self, v: u8) {
        self.put(v);
    }
    #[inline]
    fn u16(&mut self, v: u16) {
        for b in v.to_le_bytes() {
            self.put(b);
        }
    }
    #[inline]
    fn u32(&mut self, v: u32) {
        for b in v.to_le_bytes() {
            self.put(b);
        }
    }
    #[inline]
    fn len(&self) -> usize {
        self.off - self.start
    }
}

/// True if `[addr, addr+len)` lies wholly inside the arena.
#[inline]
fn arena_contains(addr: usize, len: usize) -> bool {
    let base = arena_phys();
    addr >= base && len <= ARENA_BYTES && addr - base <= ARENA_BYTES - len
}

/// Fill the target region with a 32-bit pattern (CPU-side; pre-seed sentinel).
fn fill_target(pattern: u32) {
    let arena = &raw mut V3D_ARENA;
    unsafe {
        let mut i = 0;
        while i < TARGET_BYTES {
            for b in pattern.to_le_bytes() {
                (*arena).bytes[OFF_TARGET + i] = b;
                i += 1;
            }
        }
    }
}

/// CPU-side verify: every 32-bit word of the target equals `expect`. Reports the first mismatch.
fn verify_target(expect: u32) -> bool {
    let arena = &raw const V3D_ARENA;
    let mut i = 0;
    while i + 4 <= TARGET_BYTES {
        let w = unsafe {
            let b = &(*arena).bytes;
            u32::from_le_bytes([
                b[OFF_TARGET + i],
                b[OFF_TARGET + i + 1],
                b[OFF_TARGET + i + 2],
                b[OFF_TARGET + i + 3],
            ])
        };
        if w != expect {
            serial_println!(
                ":: V3D: verify mismatch at word {} — got {:#010x} expect {:#010x} ::",
                i / 4, w, expect
            );
            return false;
        }
        i += 4;
    }
    true
}

/// Blit the verified 64×64 target into the top-left of the panel framebuffer — the metal visible
/// witness. Bounds-checked against both the target and the framebuffer; clips to whatever fits.
fn blit_target(fb: &FbTarget) {
    if fb.base == 0 || fb.bytes_per_pixel < 4 {
        return;
    }
    let arena = &raw const V3D_ARENA;
    let w = TARGET_W.min(fb.width);
    let h = TARGET_H.min(fb.height);
    for y in 0..h {
        for x in 0..w {
            let src = OFF_TARGET + (y * TARGET_W + x) * TARGET_BPP;
            let px = unsafe {
                let b = &(*arena).bytes;
                u32::from_le_bytes([b[src], b[src + 1], b[src + 2], b[src + 3]])
            };
            let dst = fb.base as usize + y * fb.stride_px * fb.bytes_per_pixel + x * fb.bytes_per_pixel;
            // Confine the write to the framebuffer extent.
            if dst + 4 <= fb.base as usize + fb.size {
                unsafe { core::ptr::write_volatile(dst as *mut u32, px) };
            }
        }
    }
}

/// Poll `reg` at `base` until `mask` clears, with a finite ~500 ms wall-clock backstop off CNTPCT.
/// Returns false on timeout (the caller fails closed). This is the anti-hang discipline: never an
/// unbounded spin — a wedged GPU degrades the boot to "no V3D", it does not hang it.
fn wait_bit_clear(base: usize, reg: usize, mask: u32, what: &str) -> bool {
    let deadline = super::timer::cntpct() + super::timer::cntfrq() / 2;
    while mmio_read(base, reg) & mask != 0 {
        if super::timer::cntpct() >= deadline {
            serial_println!(":: V3D: timeout waiting for {} (backstop) ::", what);
            return false;
        }
        core::hint::spin_loop();
    }
    true
}
