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

// ─── PI-V3D-3: the PM / ASB (AXI async bridge) enable step. ───────────────────────────────────────
// PI-V3D-2's metal verdict (2026-07-18, non-relitigable): firmware power domain 10 ACKed ON, clock id
// 5 rate 500 MHz ACKed, clock GATE ACKed active — yet the V3D hub STILL reads 0xdeadbeef (BUS-POISON,
// probe fail-closed correctly). Conclusion of record: the RPi firmware property-tag power+clock path
// is NOT sufficient to decode the V3D block on BCM2711.
//
// Adjudication (Linux `drivers/soc/bcm/bcm2835-power.c` + `arch/arm/boot/dts/bcm2711.dtsi`, rpi-6.1.y):
// on BCM2711 the V3D power domain (`BCM2835_POWER_DOMAIN_GRAFX_V3D`) is brought up by
// `bcm2835_asb_power_on(PM_GRAFX, ASB_V3D_M_CTRL, ASB_V3D_S_CTRL, PM_V3DRSTN)`. Two of its steps are
// DISTINCT from the firmware power-domain path and are the missing piece:
//   (1) deassert the V3D reset — set PM_V3DRSTN (bit 6) in PM_GRAFX, written with the PM password.
//   (2) release the two async AXI bridges — clear ASB_REQ_STOP in ASB_V3D_M_CTRL then ASB_V3D_S_CTRL
//       (each written with the PM password) and wait for ASB_ACK to clear.
// The V3D ASB registers live in the `rpivid_asb` reg block, NOT the legacy `asb` block: in the DT the
// `pm` node's third reg range is `<0x7ec11000 0x20>` "rpivid_asb", and `bcm2835_asb_control` routes
// ASB_V3D_{S,M}_CTRL to `power->rpivid_asb` when present (always, on BCM2711). The PM_POWUP/inrush/
// memory-repair sequence (`bcm2835_power_power_on`) is SKIPPED on BCM2711 (`if (power->rpivid_asb)
// return 0`) — the firmware already did it, which is why our mailbox SET_DOMAIN_STATE domain 10 ACKs.
// So we KEEP the firmware power/rate/gate steps (ACKed-working, still necessary) and ADD only the
// reset-deassert + ASB-release step, sequenced after them.
//
// Both bases are ARM PAs inside the 0xC000_0000–0xFFFF_FFFF Device-nGnRnE window already mapped by
// boot.rs L1[3] — no new MMU mapping. QEMU raspi4b models neither the rpivid_asb block nor V3D, so
// every read/write here is poison/absent-tolerant and every wait is a finite CNTPCT backstop: on QEMU
// the ASB regs are unbacked (read 0, ACK already clear → no wait, no fault), and the IDENT0 probe that
// follows still lands on the honest BLOCK-DOWN. On metal the discriminating expectation becomes
// BLOCK-UP.
const PM_BASE: usize = 0xFE10_0000; // ARM PA of the PM block (VC bus 0x7E10_0000, DT "pm")
const PM_GRAFX: usize = 0x010C; // graphics power-domain control register
const PM_V3DRSTN: u32 = 1 << 6; // deassert = V3D out of reset (bcm2835-power PM_V3DRSTN)
const PM_PASSWORD: u32 = 0x5A00_0000; // every PM (and ASB) write must carry this in the top byte

const RPIVID_ASB_BASE: usize = 0xFEC1_1000; // ARM PA of the rpivid_asb block (VC bus 0x7EC1_1000)
const ASB_V3D_S_CTRL: usize = 0x08; // V3D slave AXI bridge control
const ASB_V3D_M_CTRL: usize = 0x0C; // V3D master AXI bridge control
const ASB_REQ_STOP: u32 = 1 << 0; // request the bridge stopped (clear to release)
const ASB_ACK: u32 = 1 << 1; // bridge stopped acknowledge (clears when released)

// ─── Hub registers (offset from V3D_HUB_BASE), per v3d_regs.h. ───
const V3D_HUB_IDENT0: usize = 0x0008;
const V3D_HUB_IDENT1: usize = 0x000C;
const V3D_HUB_IDENT2: usize = 0x0010;
const V3D_HUB_IDENT3: usize = 0x0014;

// V3D MMU (in the hub), per v3d_regs.h / v3d_mmu.c. The register OFFSETS and the V3D_MMU_CTL BIT
// FIELDS below are transcribed verbatim from Linux `drivers/gpu/drm/v3d/v3d_regs.h` (torvalds/linux
// master). PI-V3D-4 root cause: the earlier constants here were fabricated. V3D_MMU_VIO_ADDR/
// DEBUG_INFO pointed at V3D_MMU_HIT (0x1208) / VIO_ADDR (0x1234) instead of the real slots, and —
// fatally — the CTL bit fields were invented at the *top* of the word (ENABLE=1<<31 …). The real
// ENABLE is BIT(0): the enable write therefore set only reserved bits, so the MMU never enabled and
// the readback (undefined/reserved bits do not latch) came back 0x00000000 — precisely the "M2 MMU
// program writes read back zero" metal symptom (R22 sitting-2). Correct layout:
//   0x1204 PT_PA_BASE · 0x1208 HIT · 0x120c MISSES · 0x1210 STALLS · 0x1214 ADDR_CAP ·
//   0x122c VIO_ID · 0x1230 ILLEGAL_ADDR · 0x1234 VIO_ADDR · 0x1238 DEBUG_INFO.
const V3D_MMUC_CONTROL: usize = 0x1000;
const V3D_MMU_CTL: usize = 0x1200;
const V3D_MMU_PT_PA_BASE: usize = 0x1204;
const V3D_MMU_VIO_ID: usize = 0x122c; // PI-V3D-5 fault-witness: id of the client that violated
const V3D_MMU_ILLEGAL_ADDR: usize = 0x1230;
const V3D_MMU_VIO_ADDR: usize = 0x1234;
const V3D_MMU_DEBUG_INFO: usize = 0x1238;

const V3D_MMUC_CONTROL_ENABLE: u32 = 1 << 0;
const V3D_MMUC_CONTROL_FLUSH: u32 = 1 << 1;

const V3D_MMU_CTL_ENABLE: u32 = 1 << 0;
const V3D_MMU_CTL_PT_INVALID_ENABLE: u32 = 1 << 16;
const V3D_MMU_CTL_PT_INVALID_ABORT: u32 = 1 << 19;
const V3D_MMU_CTL_WRITE_VIOLATION_ABORT: u32 = 1 << 11;
const V3D_MMU_CTL_TLB_CLEAR: u32 = 1 << 2;
const V3D_MMU_CTL_TLB_CLEARING: u32 = 1 << 7;
const V3D_MMU_ILLEGAL_ADDR_ENABLE: u32 = 1 << 31;
// PI-V3D-5 MMU fault-status bits (v3d_regs.h, read side of V3D_MMU_CTL — set by hardware when a
// translation faults). Used only to WITNESS a job-store fault; they change no programmed value.
const V3D_MMU_CTL_PT_INVALID: u32 = 1 << 20; // an access hit an invalid PTE
const V3D_MMU_CTL_WRITE_VIOLATION: u32 = 1 << 12; // a write hit a non-writeable page
const V3D_MMU_CTL_CAP_EXCEEDED: u32 = 1 << 27; // an access exceeded the page-table address cap

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
const V3D_CLE_CT0CS: usize = 0x0100; // CT0 (bin) control/status — witness only (render job uses CT1)
const V3D_CLE_CT1CS: usize = 0x0104; // CT1 control/status (bit5 = CTRUN busy)
const V3D_CLE_CT1CA: usize = 0x0114; // CT1 current address — the address the CLE is executing at
// PI-V3D-7 kick-path root cause: the CT1 queue-submit registers were at FABRICATED offsets. The
// begin/end addresses were written to 0x324/0x334 — not even inside the CLE register block (which
// ends at CT1QCFG 0x178). The verbatim v3d_regs.h queue slots are CT{0,1}QBA at 0x160/0x164 and
// CT{0,1}QEA at 0x168/0x16c. Writing CT1QEA is the CLE's GO signal; sending it to 0x334 meant CT1's
// real queue-end (0x16c) never fired, so the render CLE never started — CT1CA stuck at 0, CTRUN
// never latched. That is precisely the boot-P3 "never-started" signature (same fabricated-offset
// class as the PI-V3D-4 MMU-constant bug). Corrected to the transcribed offsets below.
const V3D_CLE_CT1QBA: usize = 0x0164; // CT1 queue begin address (v3d_regs.h V3D_CLE_CT1QBA)
const V3D_CLE_CT1QEA: usize = 0x016c; // CT1 queue end address (v3d_regs.h V3D_CLE_CT1QEA) — QEA write kicks
const V3D_CLE_CT1CS_CTRUN: u32 = 1 << 5; // per v3d_regs.h V3D_CLE_CTRUN

// PI-V3D-8 — CT0 (the BINNING queue). The M4 triangle first runs a BIN job on CT0 (the coordinate
// shader transforms the vertices, the PTB bins them into per-tile lists), then the RENDER job on CT1
// consumes those lists via BRANCH_TO_IMPLICIT_TILE_LIST. Every offset below is transcribed VERBATIM
// from Linux `drivers/gpu/drm/v3d/v3d_regs.h` (register offsets are hardware facts — safe to lift from
// the GPL-2.0-only header; same discipline as the PI-V3D-7 CT1 fix). NOT invented — the CT1 side is
// merely CT0+4 in every case, which the file already relies on for CT1QBA/CT1QEA.
//   0x100 CT0CS · 0x110 CT0CA · 0x160 CT0QBA · 0x168 CT0QEA · 0x170 CT0QMA · 0x174 CT0QMS.
// V3D_CLE_CT0CS is already declared above (0x0100) as the M3 witness-only register.
const V3D_CLE_CT0CA: usize = 0x0110; // CT0 current address (v3d_regs.h V3D_CLE_CT0CA)
const V3D_CLE_CT0QBA: usize = 0x0160; // CT0 queue begin address (v3d_regs.h V3D_CLE_CT0QBA)
const V3D_CLE_CT0QEA: usize = 0x0168; // CT0 queue end address (v3d_regs.h V3D_CLE_CT0QEA) — QEA write kicks
const V3D_CLE_CT0QMA: usize = 0x0170; // CT0 bin tile-state-array base (v3d_regs.h V3D_CLE_CT0QMA)
const V3D_CLE_CT0QMS: usize = 0x0174; // CT0 bin tile-state-array size  (v3d_regs.h V3D_CLE_CT0QMS)

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
/// Full system barrier — ensure prior Device-nGnRnE register writes have reached the endpoint before
/// the following readback observes their effect. Device memory is already strongly ordered, but the
/// V3D hub sits behind the async AXI bridge; a `dsb sy` makes the program→verify handoff explicit.
#[inline]
fn dsb() {
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
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
const OFF_RCL: usize = 0x8000; // [32 KiB, …) the main render control list (CT1 [BA,EA))
// PI-V3D-6: the render is a two-level control list, exactly as Mesa's v3dX(emit_rcl) builds it. The
// main list (OFF_RCL) is what CT1 executes; it branches into a *generic per-tile list* (OFF_SUBLIST)
// once per supertile via START_ADDRESS_OF_GENERIC_TILE_LIST + SUPERTILE_COORDINATES, and that sub-list
// carries the actual tile-buffer STORE. The tile-allocation scratch (OFF_TILEALLOC) is the base a
// binner would fill; our clear-only render never emits BRANCH_TO_IMPLICIT_TILE_LIST, so it is never
// dereferenced — present only because MULTICORE_RENDERING_TILE_LIST_SET_BASE requires an address.
const OFF_SUBLIST: usize = 0x9000; // generic per-tile list (branched to per supertile)
const OFF_TILEALLOC: usize = 0xA000; // tile-alloc base (inert: no binned geometry)

// ── PI-V3D-8 (M4 triangle) arena regions. All 4 KiB-aligned, all ABOVE the M3 regions so the M3
// clear-job is untouched (it must still PASS as the regression witness). Every region is inside the
// 256 KiB arena (top used byte 0x20000 < ARENA_BYTES 0x40000) and therefore inside the identity MMU
// map — a control list referencing any of these iovas is confined by the V3D MMU exactly like M3. ──
const OFF_M4_TARGET: usize = 0x0C000; // [48 KiB) the 64×64 RGBA8 the render stores the triangle into
const OFF_BIN_CL: usize = 0x10000; // binning control list (CT0 [BA,EA))
const OFF_TILESTATE: usize = 0x11000; // bin tile-state data array (CT0QMA; 48 B/tile, 1 tile here)
const OFF_BIN_TILEALLOC: usize = 0x12000; // bin tile-allocation memory (binner output; render reads it)
const OFF_M4_RCL: usize = 0x1A000; // M4 render control list (CT1 [BA,EA))
const OFF_M4_SUBLIST: usize = 0x1B000; // M4 generic per-tile list (branch-to-implicit + store)
const OFF_SHADREC: usize = 0x1C000; // GL Shader State Record (32-B aligned) + attribute record
const OFF_VTXDATA: usize = 0x1D000; // triangle vertex attribute data (3 verts × vec4 clip position)
const OFF_CS_CODE: usize = 0x1E000; // coordinate shader QPU code (binning: transform → VPM)
const OFF_VS_CODE: usize = 0x1E800; // vertex shader QPU code (render: transform + varyings → VPM)
const OFF_FS_CODE: usize = 0x1F000; // fragment shader QPU code (solid colour → TLB)
const OFF_DEFAULT_ATTRS: usize = 0x1F800; // default attribute values block (shader-record field)
const BIN_TILEALLOC_BYTES: usize = 0x8000; // 32 KiB of tile-alloc scratch for the binner

/// The solid triangle colour the fragment shader writes and the CPU verifies INSIDE the primitive.
/// Distinct from CLEAR_RGBA so the sample test can tell inside (this) from outside (clear). UnaOS
/// amber, RGBA8888 little-endian. (Exact channel order is store-config dependent, same as CLEAR_RGBA;
/// the CPU verify reads the same 32-bit word the store wrote, so the check is order-agnostic.)
const TRI_RGBA: u32 = 0x00FF_B000;


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

    // PI-V3D-3: the PM / ASB enable step — the piece the firmware power+clock path leaves undone on
    // BCM2711 (PI-V3D-2's metal verdict). Deassert the V3D reset in PM_GRAFX, then release the two
    // async AXI bridges (master, slave). Sequenced AFTER the firmware power/rate/gate steps above and
    // BEFORE the probe, so the probe reads a (hopefully) decoded block. Best-effort + poison-honest:
    // on QEMU these registers are absent and every wait is a finite backstop, so the run still lands
    // on the honest BLOCK-DOWN below; on metal this is what turns BUS-POISON into BLOCK-UP.
    enable_pm_asb();

    // Let the freshly powered + clocked + bridged block settle before its first register read (a
    // bounded wall-clock delay off CNTPCT — finite by construction, never an unbounded spin).
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
            // SError-drain class fix: the powered/clocked/bridged sequence above wrote into a block
            // that never came up — any of those accesses may have left a LATENT async external
            // abort pending (the R22 sitting-2 first-tick SERROR). Drain before returning so the
            // fail-closed branch leaves the machine clean.
            super::exceptions::serror_drain_request("v3d: BUS-POISON probe");
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
        // The MMU-program writes are the R22 sitting-2 metal offender: they targeted a block whose
        // probe passed but whose MMU window aborted, leaving a latent SError for the first DAIF
        // unmask. Drain it here so the fail-closed exit leaves the machine clean.
        super::exceptions::serror_drain_request("v3d: M2 MMU program failed");
        return;
    }
    serial_println!(":: V3D: M2 MMU PASS (arena identity-mapped, confined, TLB flushed) ::");

    // ── M3: clear job. ──────────────────────────────────────────────────────────────────────────
    if clear_job(fb) {
        serial_println!(":: V3D: M3 clear-job PASS (GPU cleared buffer; CPU byte-verified) ::");
    } else {
        serial_println!(":: V3D: M3 clear-job did not verify — see lines above ::");
    }

    // ── M4: the first triangle. Bin one triangle on CT0, render it on CT1 (implicit tile list), then
    // CPU-verify inside/outside samples. M3's PASS above is the regression witness — M4 runs AFTER it,
    // in its own arena regions, and never touches M3's buffers. ATTENDED-METAL-UNVERIFIED (QEMU raspi4b
    // never reaches here; on QEMU the run returned at BLOCK-DOWN far above). ────────────────────────
    triangle_job(fb);

    // Belt-and-suspenders for the whole bring-up: whatever path M2/M3/M4 took, no latent async abort
    // from a V3D register access may outlive this function (the SError-drain class rule).
    super::exceptions::serror_drain_request("v3d: bring-up exit");
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

/// PI-V3D-3: the PM / ASB enable step. On BCM2711 the firmware property-tag power+clock path leaves
/// the V3D held in reset with its async AXI bridges stopped (PI-V3D-2 metal: powered+clocked yet
/// 0xdeadbeef). Mirror the two BCM2711-relevant steps of Linux `bcm2835_asb_power_on` for the
/// GRAFX_V3D domain: (1) deassert PM_V3DRSTN in PM_GRAFX, (2) release ASB_V3D_M_CTRL then
/// ASB_V3D_S_CTRL. Every PM/ASB write carries the PM password. Best-effort: a bridge that never ACKs
/// (or reads poison) is logged and we proceed — the IDENT0 probe that follows is the real verdict
/// gate (it BUS-POISONs honestly if the block still did not decode). Announced-before-issue writes,
/// poison-honest readbacks, bounded settles — nothing here can fault or hang (QEMU-safe).
fn enable_pm_asb() {
    // (1) Deassert the V3D reset in PM_GRAFX (bit PM_V3DRSTN), preserving the other bits, PM password
    // in the top byte. Read-modify-write via the Device window; the read is poison-tolerant (we only
    // OR in our bit and re-stamp the password, so any bus value is harmless).
    let grafx = mmio_read(PM_BASE, PM_GRAFX);
    serial_println!(
        ":: V3D: PM/ASB deassert V3D reset — PM_GRAFX {:#010x} -> set PM_V3DRSTN (pw) ::",
        grafx
    );
    mmio_write(PM_BASE, PM_GRAFX, PM_PASSWORD | (grafx | PM_V3DRSTN));
    let grafx_rb = mmio_read(PM_BASE, PM_GRAFX);
    serial_println!(
        ":: V3D: PM_GRAFX readback {:#010x}{} ::",
        grafx_rb,
        if is_poison(grafx_rb) { " (poison/absent — QEMU or block-down)" } else { "" }
    );

    // (2) Release the two async AXI bridges: master first, then slave (Linux order). Clear ASB_REQ_STOP
    // and wait for ASB_ACK to clear, bounded.
    asb_release("V3D master (ASB_V3D_M_CTRL)", ASB_V3D_M_CTRL);
    asb_release("V3D slave  (ASB_V3D_S_CTRL)", ASB_V3D_S_CTRL);
}

/// Release one V3D async AXI bridge in the rpivid_asb block: clear ASB_REQ_STOP (with the PM password)
/// and wait, with a finite CNTPCT backstop, for ASB_ACK to clear. Announced-before-issue; poison-honest
/// readback. Never fatal — a bridge that will not release is logged and the caller proceeds to let the
/// IDENT0 probe deliver the honest verdict.
fn asb_release(what: &str, reg: usize) {
    let cur = mmio_read(RPIVID_ASB_BASE, reg);
    serial_println!(
        ":: V3D: PM/ASB release {} — cur {:#010x} -> clear ASB_REQ_STOP (pw) ::",
        what, cur
    );
    mmio_write(RPIVID_ASB_BASE, reg, PM_PASSWORD | (cur & !ASB_REQ_STOP));
    // Wait ~5 ms for ACK to clear (Linux uses 1 µs on real silicon; we are generous). On QEMU the
    // register is unbacked/reads 0, so ACK is already clear and this returns at once.
    let released = wait_bit_clear(RPIVID_ASB_BASE, reg, ASB_ACK, what);
    let rb = mmio_read(RPIVID_ASB_BASE, reg);
    serial_println!(
        ":: V3D: PM/ASB {} readback {:#010x} — {}{} ::",
        what,
        rb,
        if released { "ACK clear (bridge released)" } else { "ACK still set (backstop hit — proceeding)" },
        if is_poison(rb) { ", poison/absent (QEMU or block-down)" } else { "" }
    );
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
    let pt_base_pages = (pt_phys() >> V3D_MMU_PAGE_SHIFT) as u32;
    let ctl_want = V3D_MMU_CTL_ENABLE
        | V3D_MMU_CTL_PT_INVALID_ENABLE
        | V3D_MMU_CTL_PT_INVALID_ABORT
        | V3D_MMU_CTL_WRITE_VIOLATION_ABORT;
    let illegal_want = ((base >> V3D_MMU_PAGE_SHIFT) as u32) | V3D_MMU_ILLEGAL_ADDR_ENABLE;
    serial_println!(
        ":: V3D: MMU program — PT_PA_BASE<={:#010x} (pt@{:#x}) CTL<={:#010x} ILLEGAL_ADDR<={:#010x} ::",
        pt_base_pages, pt_phys(), ctl_want, illegal_want
    );
    mmio_write(V3D_HUB_BASE, V3D_MMU_PT_PA_BASE, pt_base_pages);
    mmio_write(V3D_HUB_BASE, V3D_MMU_CTL, ctl_want);
    // Illegal-address trap points at arena page 0 (a benign in-arena page) with the enable bit; a
    // stray access lands there instead of undefined RAM.
    mmio_write(V3D_HUB_BASE, V3D_MMU_ILLEGAL_ADDR, illegal_want);

    // Flush the MMU cache + TLB. Finite backstop on the TLB-clearing bit (never an unbounded spin).
    mmio_write(V3D_HUB_BASE, V3D_MMUC_CONTROL, V3D_MMUC_CONTROL_FLUSH | V3D_MMUC_CONTROL_ENABLE);
    mmio_write(V3D_HUB_BASE, V3D_MMU_CTL, mmio_read(V3D_HUB_BASE, V3D_MMU_CTL) | V3D_MMU_CTL_TLB_CLEAR);
    if !wait_bit_clear(V3D_HUB_BASE, V3D_MMU_CTL, V3D_MMU_CTL_TLB_CLEARING, "MMU TLB clear") {
        return false;
    }

    // Ensure the programming writes have landed at the hub before we read the state back.
    dsb();

    // Verify: MMU reports enabled (CTL.ENABLE=bit0 latched), no violation address latched. The
    // readback is now against the CORRECT offsets/bits — a live block echoes ENABLE|PT_INVALID_ENABLE|
    // aborts; the all-zero readback that fail-closed on metal was the fabricated-constants bug.
    let ctl = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let ptb = mmio_read(V3D_HUB_BASE, V3D_MMU_PT_PA_BASE);
    let vio = mmio_read(V3D_HUB_BASE, V3D_MMU_VIO_ADDR);
    let dbg = mmio_read(V3D_HUB_BASE, V3D_MMU_DEBUG_INFO);
    let enabled = ctl & V3D_MMU_CTL_ENABLE != 0;
    serial_println!(
        ":: V3D: MMU readback CTL={:#010x} (ENABLE={}) PT_PA_BASE={:#010x} VIO_ADDR={:#010x} DEBUG={:#010x} (mapped {} arena pages @ {:#x}) ::",
        ctl, enabled as u32, ptb, vio, dbg, ARENA_PAGES, base
    );
    enabled
}

/// M3: build a minimal render control list (RCL) that clears the tile buffer to CLEAR_RGBA and stores
/// it into the target buffer, kick CT1, poll to completion with a finite backstop, then have the CPU
/// byte-verify the target. On success, blit the target into the panel framebuffer (metal witness).
///
/// The RCL is a two-level render-only list (main + generic per-tile sub-list, no binner/shaders) per
/// Mesa v3d_packet_v33.xml 4.2 encodings + v3dx_rcl.c ordering — see `build_rcl`.
/// ATTENDED-METAL-UNVERIFIED: QEMU never runs this.
fn clear_job(fb: Option<FbTarget>) -> bool {
    // Pre-seed the target with a sentinel DIFFERENT from the clear colour, so a passing verify proves
    // the GPU actually wrote (not a lucky pre-existing pattern).
    fill_target(0xDEAD_BEEF);

    let (rcl_len, sublist_len) = build_rcl();
    // Publish the target (sentinel) + BOTH control lists to RAM for the non-coherent GPU. The main
    // list is what CT1 fetches; the generic per-tile sub-list is branched to per supertile, so it must
    // be published too (PI-V3D-6: the store lives in the sub-list — an unpublished sub-list is exactly
    // the "CLE ran, store landed nowhere" class-B failure this arc fixes).
    cache::clean_range(arena_phys() + OFF_TARGET, TARGET_BYTES);
    cache::clean_range(arena_phys() + OFF_RCL, rcl_len);
    cache::clean_range(arena_phys() + OFF_SUBLIST, sublist_len);

    // Kick CT1 (render queue): begin address .. end address. Both are arena-internal identity iovas,
    // bounds-checked here — the memory-safety guarantee for what the CLE fetches.
    let ba = arena_phys() + OFF_RCL;
    let ea = ba + rcl_len;
    if !arena_contains(ba, rcl_len) {
        serial_println!(":: V3D: RCL range escapes the arena — refusing kick (fail-closed) ::");
        return false;
    }
    // PI-V3D-5 job-never-ran witness (class A): snapshot the CLE status the instant BEFORE the kick,
    // so the post-kick reads have a baseline. CTRUN clearing could mean "finished" OR "never started";
    // only a before/after pair disambiguates.
    let cs_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CS);
    let ca_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CA);
    // Order matters: program the queue-BEGIN address first, then writing the queue-END address is the
    // CLE's GO trigger (v3d_regs.h / the kernel v3d_gem submit path: CT1QBA then CT1QEA). With the
    // offsets now correct, this QEA write is what actually starts CT1.
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT1QBA, ba as u32);
    dsb(); // BA must be latched before the EA write triggers the fetch
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT1QEA, ea as u32);
    dsb(); // ensure the GO (QEA) write reaches the CLE before we sample its status
    // Tight kick witness: sample CT1CS + CT1CA the instant after the GO write. A started CLE latches
    // CTRUN here and CT1CA leaves 0/BA to walk the list; a never-started CLE shows CTRUN=0 and CT1CA
    // unchanged from ca_pre. This pair is the boot-P4 discriminator's ground truth.
    let cs_kicked = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CS);
    let ca_kicked = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CA);

    // Poll for CT1 idle (CTRUN clears when the list finishes) with a finite ~500 ms backstop.
    let idled = wait_bit_clear(V3D_CORE0_BASE, V3D_CLE_CT1CS, V3D_CLE_CT1CS_CTRUN, "CT1 render");

    // PI-V3D-5 two-class witness block. Read the CLE progress + the V3D MMU fault status BEFORE the
    // verify, so the metal log tells job-never-ran (class A) from job-ran-but-wrote-elsewhere/faulted
    // (class B) regardless of what the verify then reports. All reads; nothing here is programmed.
    let cs_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CS);
    let ct0cs = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    let ct1ca = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CA);
    let mmu_ctl = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let vio_addr = mmio_read(V3D_HUB_BASE, V3D_MMU_VIO_ADDR);
    let vio_id = mmio_read(V3D_HUB_BASE, V3D_MMU_VIO_ID);
    let mmu_fault = mmu_ctl
        & (V3D_MMU_CTL_PT_INVALID | V3D_MMU_CTL_WRITE_VIOLATION | V3D_MMU_CTL_CAP_EXCEEDED);
    // PI-V3D-7 discriminator fix. The old `ran` test used `ct1ca != BA` as proof of execution — but a
    // CLE that NEVER STARTED also satisfies that, because its CT1CA reads 0 (≠ the non-zero BA). That
    // false-positive mislabeled boot-P3's never-started CLE as CLASS-B RAN-NO-FAULT. Correct truth
    // table: the CLE ran ONLY if we ever OBSERVED CTRUN set (in any of pre/kicked/done) OR CT1CA
    // actually ADVANCED — i.e. it points INTO the list range (BA, EA] rather than sitting at 0 or BA.
    // A never-started CLE has CTRUN never seen AND CT1CA at 0/BA → CLASS-A.
    let ctrun_ever = (cs_pre | cs_kicked | cs_done) & V3D_CLE_CT1CS_CTRUN != 0;
    let ct1ca_advanced =
        ct1ca != 0 && ct1ca != ba as u32 && ct1ca >= ba as u32 && ct1ca <= ea as u32;
    let ran = ctrun_ever || ct1ca_advanced;
    // PI-V3D-8 mislabel fix. The old "RAN-NO-FAULT" else-branch asserted "store landed off-target"
    // UNCONDITIONALLY — so even a SUCCESSFUL run (CLE ran, no fault, store correct, verify passes) was
    // clued as a class-B failure. Do the verify FIRST (only meaningful once the CLE has idled) and let
    // its result pick the label: a verified store is RAN-OK, an unverified one is the genuine class-B
    // off-target case. The old code did the invalidate+verify AFTER this block; it now lives here and
    // the later return simply reuses the result.
    let verified = if idled {
        cache::clean_invalidate_range(arena_phys() + OFF_TARGET, TARGET_BYTES);
        Some(verify_target(CLEAR_RGBA))
    } else {
        None
    };
    let class = if mmu_fault != 0 {
        "CLASS-B MMU-FAULT (store faulted in the V3D MMU — job wrote nowhere)"
    } else if !ran {
        "CLASS-A JOB-NEVER-RAN (CTRUN never observed AND CT1CA never advanced from 0/BA — CLE did not start)"
    } else if !idled {
        "INDETERMINATE (CLE started but CTRUN never cleared — backstop hit)"
    } else if verified == Some(true) {
        "RAN-OK (CLE executed, no MMU fault, store byte-verified)"
    } else {
        "CLASS-B RAN-NO-FAULT (CLE executed with no MMU fault — store landed off-target: RCL encoding)"
    };
    serial_println!(
        ":: V3D: M3 clue — CT1CS pre={:#010x} kicked={:#010x} done={:#010x} CT1CA pre={:#010x} kicked={:#010x} done={:#010x} CT0CS={:#010x} (BA={:#010x} EA={:#010x}) ran={} — {} ::",
        cs_pre, cs_kicked, cs_done, ca_pre, ca_kicked, ct1ca, ct0cs, ba as u32, ea as u32, ran as u32, class
    );
    serial_println!(
        ":: V3D: M3 clue — MMU_CTL={:#010x} (PT_INVALID={} WRITE_VIOLATION={} CAP_EXCEEDED={}) VIO_ADDR={:#010x} VIO_ID={:#010x} ::",
        mmu_ctl,
        (mmu_ctl & V3D_MMU_CTL_PT_INVALID != 0) as u32,
        (mmu_ctl & V3D_MMU_CTL_WRITE_VIOLATION != 0) as u32,
        (mmu_ctl & V3D_MMU_CTL_CAP_EXCEEDED != 0) as u32,
        vio_addr, vio_id
    );
    // SError-drain correlation witness: a V3D store that faulted on the bus can leave a latent async
    // external abort that the global SError-drain would otherwise consume silently at bring-up exit
    // (or, worse, at the first timer tick). Drain it HERE, labelled to this exact kick→poll window, so
    // the "consumed N latent async abort(s)" line — if any — is unambiguously correlated with the M3
    // clear-job store, not with M1/M2. Zero drained here = the store did not raise a bus fault.
    super::exceptions::serror_drain_request("v3d: M3 clear-job kick window");

    if !idled {
        serial_println!(":: V3D: CT1 did not idle within budget — no verify (anti-hang backstop hit) ::");
        return false;
    }

    // The GPU's writes were already read back and byte-verified above (the `verified` snapshot the
    // clue label used — the clean_invalidate there is what forces DRAM truth, defeating a stale-CPU-line
    // false negative). Reuse it; on success blit the target to the panel (metal visible witness).
    let ok = verified == Some(true);
    if ok {
        if let Some(fb) = fb {
            blit_target(&fb);
        }
    }
    ok
}

// ─── V3D 4.2 (BCM2711) control-list packet encodings ──────────────────────────────────────────────
// PI-V3D-6: the placeholder that boot-P2 convicted (CLASS-B RAN-NO-FAULT) wrote a 0x1a-byte stream of
// bare opcode bytes with NO field packing, several WRONG opcodes (114 for "clear colors" — actually
// Blend Enables; 125 for "end-of-tile" — actually Tile Coordinates Implicit), a STORE with the target
// address at the wrong byte offset and no format/stride/buffer fields, and — fatally — NO
// SUPERTILE_COORDINATES, so nothing ever triggered a tile store. The CLE happily ran the malformed
// bytes to completion (no MMU fault) and wrote nowhere. This is the correct encoding.
//
// All opcodes, field bit-positions, sizes, enum values and packet lengths below are transcribed
// verbatim from Mesa `src/broadcom/cle/v3d_packet_v33.xml` (`gen="3.3" max_ver="42"`, the V3D 4.2
// variants) and the emission ORDER follows Mesa `src/gallium/drivers/v3d/v3dx_rcl.c`
// (`v3dX(emit_rcl)` + `emit_render_layer` + `v3d_rcl_emit_generic_per_tile_list`). Mesa is
// MIT-licensed — verbatim-liftable WITH attribution (memory: unaos-license-gplv3). No Linux-kernel
// (GPL-2.0-only) v3d source is used here.
//
// Packing convention (Mesa `gen_pack_header.py`): byte 0 is the opcode; every XML `start` bit is
// relative to the bit AFTER the opcode, i.e. absolute packet bit = XML start + 8. Packet length =
// max(field end bit)/8 + 1 bytes. `set_bits` writes a field LSB-first at its absolute bit.

// Packet opcodes (v3d_packet_v33.xml `code=`).
const P_TRMC: u8 = 121; // Tile Rendering Mode Cfg (sub-id field selects Common/Color/Clear/ZS variant)
const P_TILE_COORDINATES: u8 = 124;
const P_TILE_COORDINATES_IMPLICIT: u8 = 125;
const P_STORE_TILE_BUFFER_GENERAL: u8 = 29;
const P_CLEAR_TILE_BUFFERS: u8 = 25;
const P_END_OF_LOADS: u8 = 26;
const P_END_OF_TILE_MARKER: u8 = 27;
const P_FLUSH_VCD_CACHE: u8 = 19;
const P_GENERIC_TILE_LIST: u8 = 20; // Start Address of Generic Tile List
const P_RETURN_FROM_SUB_LIST: u8 = 18;
const P_PRIM_LIST_FORMAT: u8 = 56;
const P_SET_INSTANCEID: u8 = 54;
const P_TILE_LIST_INITIAL_BLOCK_SIZE: u8 = 126;
const P_MULTICORE_TILE_LIST_BASE: u8 = 123; // Multicore Rendering Tile List Set Base
const P_MULTICORE_SUPERTILE_CFG: u8 = 122;
const P_SUPERTILE_COORDINATES: u8 = 23;
const P_END_OF_RENDERING: u8 = 0; // Halt (Mesa END_OF_RENDERING)

// TILE_RENDERING_MODE_CFG sub-ids (v42 `sub-id` field defaults).
const TRMC_SUBID_COMMON: u64 = 0;
const TRMC_SUBID_COLOR: u64 = 1;
const TRMC_SUBID_ZS_CLEAR_VALUES: u64 = 2;
const TRMC_SUBID_CLEAR_COLORS_PART1: u64 = 3;

// Internal-format enum values (v3d_packet_v33.xml enums). rgba8 unorm render target: 32-bit internal
// BPP (Internal BPP "32" = 0), internal type "8" = 2; stored Output Image Format rgba8 = 27.
const INTERNAL_BPP_32: u64 = 0;
const INTERNAL_TYPE_8: u64 = 2;
const OUTPUT_IMAGE_FORMAT_RGBA8: u64 = 27;
const MEMORY_FORMAT_RASTER: u64 = 0;
const PRIM_TYPE_LIST_TRIANGLES: u64 = 2;
const TILE_ALLOC_BLOCK_SIZE_64B: u64 = 0;

/// Build BOTH control lists (main at OFF_RCL, generic per-tile sub-list at OFF_SUBLIST) and publish the
/// sub-list to RAM for the non-coherent GPU. Returns `(main_len, sublist_len)` in bytes; the caller
/// kicks CT1 over `[OFF_RCL, OFF_RCL+main_len)` and has already published the main list + target.
///
/// Shape (single 64×64 tile = single supertile, no binned geometry — a pure clear+store), per Mesa
/// `v3dX(emit_rcl)`. ATTENDED-METAL-UNVERIFIED: QEMU raspi4b models no V3D, so this is
/// correct-by-construction against the cited Mesa sources, refined at the attended sitting.
fn build_rcl() -> (usize, usize) {
    let target = (arena_phys() + OFF_TARGET) as u32;
    let sublist_start = (arena_phys() + OFF_SUBLIST) as u32;
    let tile_alloc = (arena_phys() + OFF_TILEALLOC) as u32;
    let stride = (TARGET_W * TARGET_BPP) as u64; // raster row stride in bytes (64 px × 4 B = 256)

    // ── Generic per-tile sub-list (OFF_SUBLIST) — executed once per SUPERTILE_COORDINATES. Carries the
    // real tile-buffer STORE. Order per v3d_rcl_emit_generic_per_tile_list (V3D >= 41 path). We omit
    // BRANCH_TO_IMPLICIT_TILE_LIST: there is no binned geometry, so no implicit tile list to run. ──
    let mut s = RclWriter::new(OFF_SUBLIST);
    s.pkt(Pkt::new(P_TILE_COORDINATES_IMPLICIT, 1).done()); // single coords; END_OF_LOADS flips load→render
    s.pkt(Pkt::new(P_END_OF_LOADS, 1).done());
    // PTB assumes triangles as the initial primitive mode; SET_INSTANCEID(0) — hw does not default it.
    s.pkt(Pkt::new(P_PRIM_LIST_FORMAT, 2).f(0, 6, PRIM_TYPE_LIST_TRIANGLES).done());
    s.pkt(Pkt::new(P_SET_INSTANCEID, 5).f(0, 32, 0).done());
    // STORE_TILE_BUFFER_GENERAL: RT0 → target, raster, rgba8, row stride. Address is a full 32-bit
    // field at XML start 64 (packet byte 9) — the exact slot the placeholder missed. This is the write
    // the CPU verifies.
    s.pkt(
        Pkt::new(P_STORE_TILE_BUFFER_GENERAL, 13)
            .f(0, 4, 0) // Buffer to Store = Render target 0
            .f(4, 3, MEMORY_FORMAT_RASTER)
            .f(12, 6, OUTPUT_IMAGE_FORMAT_RGBA8)
            .f(28, 20, stride) // Height in UB or Stride (raster → byte stride)
            .f(64, 32, target as u64) // Address
            .done(),
    );
    // GFXH-1461/1689: after the per-buffer store, clear the tile buffers (job->clear set).
    s.pkt(
        Pkt::new(P_CLEAR_TILE_BUFFERS, 2)
            .f(0, 1, 1) // Clear all Render Targets
            .f(1, 1, 1) // Clear Z/Stencil Buffer
            .done(),
    );
    s.pkt(Pkt::new(P_END_OF_TILE_MARKER, 1).done());
    s.pkt(Pkt::new(P_RETURN_FROM_SUB_LIST, 1).done());
    let sublist_len = s.len();
    let sublist_end = sublist_start + sublist_len as u32;
    // Publish the sub-list now (the main list + target are published by the caller).
    cache::clean_range(arena_phys() + OFF_SUBLIST, sublist_len);

    // ── Main render control list (OFF_RCL) — what CT1 executes. Frame config first (COMMON must be the
    // first TILE_RENDERING_MODE_CFG, ZS_CLEAR_VALUES last), then the per-layer render. ──
    let mut w = RclWriter::new(OFF_RCL);

    // TILE_RENDERING_MODE_CFG (Common): 64×64 frame, 1 render target (minus_one → 0), 32-bit max BPP,
    // no MSAA, no double-buffer, Early-Z LT/LE, depth type 0.
    w.pkt(
        Pkt::new(P_TRMC, 9)
            .f(0, 4, TRMC_SUBID_COMMON)
            .f(4, 4, 0) // Number of Render Targets (minus_one: 1 RT → 0)
            .f(8, 16, TARGET_W as u64) // Image Width (pixels)
            .f(24, 16, TARGET_H as u64) // Image Height (pixels)
            .f(40, 2, INTERNAL_BPP_32) // Maximum BPP of all render targets
            .done(),
    );
    // TILE_RENDERING_MODE_CFG (Clear Colors Part1): RT0 clear value low 32 bits = CLEAR_RGBA. For a
    // 32-bit-BPP target only Part1 is needed (Mesa emits Part2/Part3 only for >= 64/128-bit BPP).
    w.pkt(
        Pkt::new(P_TRMC, 9)
            .f(0, 4, TRMC_SUBID_CLEAR_COLORS_PART1)
            .f(4, 4, 0) // Render Target number
            .f(8, 32, CLEAR_RGBA as u64) // Clear Color low 32 bits
            .done(),
    );
    // TILE_RENDERING_MODE_CFG (Color, v42): RT0 = 32-bit BPP, internal type "8" (rgba8 unorm), no clamp.
    w.pkt(
        Pkt::new(P_TRMC, 9)
            .f(0, 4, TRMC_SUBID_COLOR)
            .f(4, 2, INTERNAL_BPP_32) // Render Target 0 Internal BPP
            .f(6, 4, INTERNAL_TYPE_8) // Render Target 0 Internal Type
            .f(10, 2, 0) // Render Target 0 Clamp = none
            .done(),
    );
    // TILE_RENDERING_MODE_CFG (ZS Clear Values) — ends the rendering-mode config. No Z/S buffer; clear
    // values are inert but the packet must terminate config.
    w.pkt(
        Pkt::new(P_TRMC, 9)
            .f(0, 4, TRMC_SUBID_ZS_CLEAR_VALUES)
            .f(8, 8, 0) // Stencil Clear Value
            .f(16, 32, 0) // Z Clear Value
            .done(),
    );
    // TILE_LIST_INITIAL_BLOCK_SIZE — must precede the first branch; auto-chained, 64-byte first block.
    w.pkt(
        Pkt::new(P_TILE_LIST_INITIAL_BLOCK_SIZE, 2)
            .f(0, 2, TILE_ALLOC_BLOCK_SIZE_64B) // Size of first block
            .f(2, 1, 1) // Use auto-chained tile lists
            .done(),
    );

    // Per-layer render (single layer). MULTICORE_RENDERING_TILE_LIST_SET_BASE: the tile-alloc base (64-
    // byte-aligned). Address field is 26 bits at XML start 6 → the 64-aligned address's bits [6..31].
    w.pkt(
        Pkt::new(P_MULTICORE_TILE_LIST_BASE, 5)
            .f(0, 4, 0) // Tile List Set Number
            .f(6, 26, (tile_alloc >> 6) as u64) // address (64-byte aligned)
            .done(),
    );
    // MULTICORE_RENDERING_SUPERTILE_CFG: 1×1 tiles, one 1×1 supertile, single core, one bin tile list.
    w.pkt(
        Pkt::new(P_MULTICORE_SUPERTILE_CFG, 9)
            .f(0, 8, 0) // Supertile Width in Tiles (minus_one: 1 → 0)
            .f(8, 8, 0) // Supertile Height in Tiles (minus_one: 1 → 0)
            .f(16, 8, 1) // Total Frame Width in Supertiles
            .f(24, 8, 1) // Total Frame Height in Supertiles
            .f(32, 12, 1) // Total Frame Width in Tiles
            .f(44, 12, 1) // Total Frame Height in Tiles
            .f(61, 3, 0) // Number of Bin Tile Lists (minus_one: 1 → 0)
            .done(),
    );

    // Initial tile-buffer clear (also the GFXH-1742 double-dummy-store workaround on V3D 4.x). Clears
    // the tile buffer to the clear color before the first tile inherits stale contents.
    w.pkt(
        Pkt::new(P_TILE_COORDINATES, 4)
            .f(0, 12, 0) // tile column number
            .f(12, 12, 0) // tile row number
            .done(),
    );
    for i in 0..2 {
        if i > 0 {
            w.pkt(
                Pkt::new(P_TILE_COORDINATES, 4).f(0, 12, 0).f(12, 12, 0).done(),
            );
        }
        w.pkt(Pkt::new(P_END_OF_LOADS, 1).done());
        // STORE (Buffer to Store = None = 8) — the dummy store that latches TLB type/size.
        w.pkt(Pkt::new(P_STORE_TILE_BUFFER_GENERAL, 13).f(0, 4, 8).done());
        if i == 0 {
            w.pkt(
                Pkt::new(P_CLEAR_TILE_BUFFERS, 2)
                    .f(0, 1, 1) // Clear all Render Targets
                    .f(1, 1, 1) // Clear Z/Stencil Buffer
                    .done(),
            );
        }
        w.pkt(Pkt::new(P_END_OF_TILE_MARKER, 1).done());
    }
    w.pkt(Pkt::new(P_FLUSH_VCD_CACHE, 1).done());

    // Branch target for the generic per-tile list, then execute the single supertile (this runs the
    // sub-list, which performs the real store), then halt.
    w.pkt(
        Pkt::new(P_GENERIC_TILE_LIST, 9)
            .f(0, 32, sublist_start as u64) // start
            .f(32, 32, sublist_end as u64) // end
            .done(),
    );
    w.pkt(
        Pkt::new(P_SUPERTILE_COORDINATES, 3)
            .f(0, 8, 0) // column number in supertiles
            .f(8, 8, 0) // row number in supertiles
            .done(),
    );
    w.pkt(Pkt::new(P_END_OF_RENDERING, 1).done());

    (w.len(), sublist_len)
}

/// A fixed-capacity control-list packet: byte 0 is the opcode, the rest is the field payload. Fields
/// are packed by their v3d_packet_v33.xml bit position via `f` (absolute packet bit = XML start + 8,
/// per Mesa's opcode-shift convention). `len` is the packet's exact byte length.
struct Pkt {
    buf: [u8; 16],
    len: usize,
}
impl Pkt {
    #[inline]
    fn new(opcode: u8, len: usize) -> Self {
        let mut buf = [0u8; 16];
        buf[0] = opcode;
        Pkt { buf, len }
    }
    /// Pack a field: `xml_start` is the v3d_packet_v33.xml `start` bit; the opcode-shift (+8) is applied
    /// here so callers can quote XML offsets verbatim.
    #[inline]
    fn f(&mut self, xml_start: usize, width: usize, val: u64) -> &mut Self {
        set_bits(&mut self.buf, xml_start + 8, width, val);
        self
    }
    #[inline]
    fn done(&self) -> (&[u8], usize) {
        (&self.buf, self.len)
    }
}

/// Write `width` bits of `val` (LSB-first) into `buf` starting at absolute bit `bit`.
#[inline]
fn set_bits(buf: &mut [u8], mut bit: usize, mut width: usize, val: u64) {
    let mut v = val;
    while width > 0 {
        let byte = bit / 8;
        let off = bit % 8;
        let take = core::cmp::min(8 - off, width);
        let mask = ((1u64 << take) - 1) as u8;
        buf[byte] |= ((v as u8) & mask) << off;
        v >>= take;
        bit += take;
        width -= take;
    }
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
            return; // saturating — never writes past the arena; the control lists are far smaller
        }
        unsafe {
            (*(&raw mut V3D_ARENA)).bytes[self.off] = b;
        }
        self.off += 1;
    }
    /// Append one encoded packet's exact bytes (`(&buf, len)` from `Pkt::done`).
    #[inline]
    fn pkt(&mut self, packet: (&[u8], usize)) {
        let (buf, len) = packet;
        for &b in &buf[..len] {
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

// ════════════════════════════════════════════════════════════════════════════════════════════════
// PI-V3D-8 — M4: the first triangle (bin on CT0 → render on CT1 → CPU sample-verify).
// ════════════════════════════════════════════════════════════════════════════════════════════════
//
// M4 adds the BINNING side of the pipeline (CT0), which M1–M3 never exercised, then a render pass that
// CONSUMES the binner's per-tile lists via BRANCH_TO_IMPLICIT_TILE_LIST. The shape (single 64×64 tile,
// one supertile, one triangle) is the minimal thing that puts real geometry + shaders through the GPU.
//
// ── PACKET FACTS (CL side — fully cited, correct-by-construction) ────────────────────────────────
// All binning-side opcodes / field bit-layouts below are transcribed VERBATIM from Mesa
// `src/broadcom/cle/v3d_packet.xml` (gen 4.2, `min_ver="42"` — the V3D 4.2 variants; identical to the
// v3d_packet_v33.xml `max_ver=42` set the M3 render list uses). Emission ORDER follows Mesa
// `src/gallium/drivers/v3d/v3dx_draw.c` (`v3dX(start_binning)` prologue + `v3dX(draw_vbo)` draw emit)
// and `v3dx_rcl.c` for the render side. Mesa is MIT-licensed — verbatim-liftable WITH attribution
// (memory: unaos-license-gplv3). No Linux-kernel (GPL-2.0-only) CLE source is used; only register
// OFFSETS are lifted from the kernel v3d_regs.h (hardware facts).
//
// ── QPU SHADER FACTS (the metal-refinement surface — honestly flagged) ───────────────────────────
// A binned+rendered triangle needs THREE QPU programs: a COORDINATE shader (binning: transform the
// vertices, write clip/screen coords to the VPM so the PTB can bin them), a VERTEX shader (render:
// same transform + emit varyings), and a FRAGMENT shader (write the solid colour to the TLB). Mesa
// COMPILES these from NIR through its VIR→QPU backend; it does not ship pre-assembled blobs, and QEMU
// `raspi4b` models no V3D, so NONE of this can be exercised or byte-checked off-metal. Rather than
// FABRICATE QPU words (the exact trap that convicted PI-V3D-4's MMU constants and PI-V3D-7's queue
// offsets — twice), the QPU programs below are built through a packer whose bit-layout is transcribed
// from Mesa `src/broadcom/qpu/qpu_pack.c` and SELF-CHECKED against Mesa's canonical NOP word
// (`0x3c003186bb800000`). The functional bodies are documented minimal skeletons; producing the
// verified transform/colour instructions is a real V3D-shader-compile step at the attended sitting.
// This is M4's ONE code-complete-prior-to-metal seam — the CL/register/state scaffolding around it is
// complete and cited. `triangle_job` witnesses the CT0 bin discriminator and the CT1 render regardless,
// and the CPU sample-verify reports exactly which samples matched, so the sitting is decisive.

// ─── Binning-side + shared packet opcodes (v3d_packet.xml `code=`). ───
const P_FLUSH: u8 = 4; // Flush — terminates the binning list (binner-done signal)
const P_START_TILE_BINNING: u8 = 6; // must follow the bin-mode config before geometry
const P_BRANCH_TO_IMPLICIT_TILE_LIST: u8 = 21; // render: run the binner's per-tile list for this tile
const P_VERTEX_ARRAY_PRIMS: u8 = 36; // non-indexed draw
const P_GL_SHADER_STATE: u8 = 64; // points at the GL Shader State Record + attribute records
const P_VCM_CACHE_SIZE: u8 = 71;
const P_NUMBER_OF_LAYERS: u8 = 119;
const P_TILE_BINNING_MODE_CFG: u8 = 120; // v42 variant (max_ver=42)

const V3D_PRIM_TRIANGLES: u64 = 4; // VERTEX_ARRAY_PRIMS "mode" (enum Primitive) — NOT the PRIM_LIST value
const TILE_STATE_BYTES: usize = 48 * 4; // TSDA: 48 B/tile, generous for the single 64×64 tile

// ─── The minimal QPU packer (V3D 4.x / VideoCore VI). ───
// Field shifts VERBATIM from Mesa `qpu_pack.c`: OP_MUL[63:58] SIG[57:53] COND[52:46] MM(45) MA(44)
// WADDR_M[43:38] WADDR_A[37:32] OP_ADD[31:24] MUL_B[23:21] MUL_A[20:18] ADD_B[17:15] ADD_A[14:12]
// RADDR_A[11:6] RADDR_B[5:0]. Opcode values from `qpu_pack.c`: add-NOP op=187 (mux a=0,b=0); mul-NOP
// op=15 (mux b=4); WADDR_NOP=6, WADDR_TLB=7. MM=MA=1 mark the write registers "magic" (Mesa sets both
// even in its NOP — which is why the canonical NOP is 0x3c003186bb800000, not …0186…).
const QPU_A_NOP: u64 = 187;
const QPU_M_NOP_OPMUL: u64 = 15;
const QPU_M_NOP_MUXB: u64 = 4;
const QPU_WADDR_NOP: u64 = 6;

/// The canonical V3D 4.x NOP instruction, derived from fields and equal to Mesa's `0x3c003186bb800000`.
const fn qpu_nop() -> u64 {
    (QPU_M_NOP_OPMUL << 58) // OP_MUL = mul NOP
        | (1u64 << 45) // MM (magic mul write)
        | (1u64 << 44) // MA (magic add write)
        | (QPU_WADDR_NOP << 38) // WADDR_M = nop
        | (QPU_WADDR_NOP << 32) // WADDR_A = nop
        | (QPU_A_NOP << 24) // OP_ADD = add NOP
        | (QPU_M_NOP_MUXB << 21) // MUL_B mux
}
const _: () = assert!(qpu_nop() == 0x3c00_3186_bb80_0000);

/// Build one of the three QPU shader programs into the arena at `off`. Documented minimal skeleton:
/// a run of NOPs (each the field-validated canonical word) that safely runs to program end — the
/// STRUCTURAL placeholder for the real transform/colour body assembled at the metal sitting. Returns
/// the byte length. Every word is written little-endian (QPU fetch order).
fn write_shader_stub(off: usize, words: usize) -> usize {
    for i in 0..words {
        arena_write_u64(off + i * 8, qpu_nop());
    }
    words * 8
}

/// Store a little-endian u64 into the arena.
#[inline]
fn arena_write_u64(off: usize, v: u64) {
    let bytes = v.to_le_bytes();
    let arena = &raw mut V3D_ARENA;
    unsafe {
        for (i, b) in bytes.iter().enumerate() {
            (*arena).bytes[off + i] = *b;
        }
    }
}
/// Store a little-endian u32 into the arena.
#[inline]
fn arena_write_u32(off: usize, v: u32) {
    let bytes = v.to_le_bytes();
    let arena = &raw mut V3D_ARENA;
    unsafe {
        for (i, b) in bytes.iter().enumerate() {
            (*arena).bytes[off + i] = *b;
        }
    }
}
/// Copy raw bytes into the arena at `off` (bounded — saturates at the arena end, never overruns).
fn arena_write_bytes(off: usize, src: &[u8]) {
    let arena = &raw mut V3D_ARENA;
    unsafe {
        for (i, b) in src.iter().enumerate() {
            if off + i >= ARENA_BYTES {
                break;
            }
            (*arena).bytes[off + i] = *b;
        }
    }
}
/// Fill a 32-bit pattern across `len` bytes at `off` (CPU-side sentinel pre-seed).
fn fill_region(off: usize, len: usize, pattern: u32) {
    let p = pattern.to_le_bytes();
    let arena = &raw mut V3D_ARENA;
    unsafe {
        let mut i = 0;
        while i < len {
            (*arena).bytes[off + i] = p[i & 3];
            i += 1;
        }
    }
}

/// The triangle's three clip-space vertices, each a vec4 (x, y, z, w) IEEE-754 f32. NDC in [-1,1];
/// a centred triangle so its interior samples land near (32,32) of the 64×64 target and its exterior
/// samples land in the corners. The COORDINATE shader is responsible for the viewport transform to the
/// 64×64 screen (its exact math is part of the metal-refined shader body). Attribute 0, stride 16 B.
const TRI_VERTS: [[f32; 4]; 3] = [
    [-0.6, -0.6, 0.5, 1.0], // lower-left
    [0.6, -0.6, 0.5, 1.0],  // lower-right
    [0.0, 0.6, 0.5, 1.0],   // top-centre
];

/// Emit one field (LSB-first) into a raw struct buffer at ABSOLUTE bit `start` — like `set_bits`, but
/// for a memory STRUCT (GL Shader State Record / attribute record) which has NO leading opcode byte, so
/// XML `start` bits are used directly (no +8 shift). Address fields whose XML size is < 32 carry the
/// aligned address already shifted by the caller.
#[inline]
fn sf(buf: &mut [u8], start: usize, width: usize, val: u64) {
    set_bits(buf, start, width, val);
}

/// Build the GL Shader State Record (v42, 36 bytes) at OFF_SHADREC and one GL Shader State Attribute
/// Record (16 bytes) immediately after it. Layout VERBATIM from Mesa `v3d_packet.xml` struct
/// "GL Shader State Record" (max_ver=42) + "GL Shader State Attribute Record"; field values follow
/// `v3dx_draw.c` `v3dX(draw_vbo)`'s shader-record emit for a trivial 1-attribute solid draw. Returns
/// the number of attribute arrays (for the GL_SHADER_STATE packet). Code addresses are 29-bit fields at
/// the top of a 32-bit aligned word (low 3 bits are the threadability/nan flags) → the address is
/// written pre-shifted `>> 3`.
fn build_shader_record() -> u32 {
    let cs = (arena_phys() + OFF_CS_CODE) as u64;
    let vs = (arena_phys() + OFF_VS_CODE) as u64;
    let fs = (arena_phys() + OFF_FS_CODE) as u64;
    let defaults = (arena_phys() + OFF_DEFAULT_ATTRS) as u64;
    let vtx = (arena_phys() + OFF_VTXDATA) as u64;

    let mut rec = [0u8; 36];
    sf(&mut rec, 1, 1, 1); // Enable clipping
    // FS: 0 varyings (solid colour). VPM segment sizes: 1 segment each (minimal); the coordinate/vertex
    // shaders each get a single input+output VPM block. These are conservative minimal values, refined
    // with the real shader's prog_data at the sitting.
    sf(&mut rec, 24, 8, 0); // Number of varyings in Fragment Shader
    sf(&mut rec, 32, 4, 1); // Coord Shader output VPM segment size
    sf(&mut rec, 40, 4, 1); // Coord Shader input VPM segment size
    sf(&mut rec, 48, 4, 1); // Vertex Shader output VPM segment size
    sf(&mut rec, 56, 4, 1); // Vertex Shader input VPM segment size
    sf(&mut rec, 64, 32, defaults); // Address of default attribute values
    // Fragment shader: flags at 96/97/98 (4-way threadable, final section, propagate NaNs), addr@99(29).
    sf(&mut rec, 96, 1, 1); // FS 4-way threadable
    sf(&mut rec, 98, 1, 1); // FS propagate NaNs (v42)
    sf(&mut rec, 99, 29, fs >> 3); // FS code address
    sf(&mut rec, 128, 32, 0); // FS uniforms address (none)
    sf(&mut rec, 160, 1, 1); // VS 4-way threadable
    sf(&mut rec, 162, 1, 1); // VS propagate NaNs (v42)
    sf(&mut rec, 163, 29, vs >> 3); // VS code address
    sf(&mut rec, 192, 32, 0); // VS uniforms address
    sf(&mut rec, 224, 1, 1); // CS 4-way threadable
    sf(&mut rec, 226, 1, 1); // CS propagate NaNs (v42)
    sf(&mut rec, 227, 29, cs >> 3); // CS code address
    sf(&mut rec, 256, 32, 0); // CS uniforms address
    arena_write_bytes(OFF_SHADREC, &rec);

    // One attribute record (vec4 position, f32), immediately after the 36-byte record.
    let mut attr = [0u8; 16];
    sf(&mut attr, 0, 32, vtx); // Address
    sf(&mut attr, 32, 2, 3); // Vec size (encodes 4 components: 4-1)
    sf(&mut attr, 34, 3, 2); // Type = Attribute float
    sf(&mut attr, 40, 4, 4); // Number of values read by Coordinate shader
    sf(&mut attr, 44, 4, 4); // Number of values read by Vertex shader
    sf(&mut attr, 64, 32, 16); // Stride (bytes per vertex)
    sf(&mut attr, 96, 32, 0xFFFF); // Maximum Index
    arena_write_bytes(OFF_SHADREC + 36, &attr);

    1 // one attribute array
}

/// Build the BINNING control list (CT0) at OFF_BIN_CL. Prologue per `v3dX(start_binning)`, draw emit
/// per `v3dX(draw_vbo)`. Returns its byte length.
fn build_bin_cl(num_attrs: u32) -> usize {
    let shadrec = (arena_phys() + OFF_SHADREC) as u32;
    let mut w = RclWriter::new(OFF_BIN_CL);

    // NUMBER_OF_LAYERS (single layer → minus_one 0), required before the bin-mode config.
    w.pkt(Pkt::new(P_NUMBER_OF_LAYERS, 2).f(0, 8, 0).done());
    // TILE_BINNING_MODE_CFG (v42): 64×64 frame, 1 RT, no MSAA, no double-buffer, 32-bit max BPP, 64-byte
    // initial + overflow tile-alloc blocks. Field bits: width@32(16,minus_one), height@48(16,minus_one),
    // num RT@8(4,minus_one), max bpp@12(2), block size@4(2), initial block size@2(2).
    w.pkt(
        Pkt::new(P_TILE_BINNING_MODE_CFG, 9)
            .f(2, 2, TILE_ALLOC_BLOCK_SIZE_64B) // tile allocation initial block size 64b
            .f(4, 2, TILE_ALLOC_BLOCK_SIZE_64B) // tile allocation block size 64b
            .f(8, 4, 0) // Number of Render Targets (minus_one: 1 → 0)
            .f(12, 2, INTERNAL_BPP_32) // Maximum BPP of all render targets
            .f(32, 16, (TARGET_W - 1) as u64) // Width in pixels (minus_one)
            .f(48, 16, (TARGET_H - 1) as u64) // Height in pixels (minus_one)
            .done(),
    );
    // Flush any stale VCD, then START_TILE_BINNING (must precede geometry).
    w.pkt(Pkt::new(P_FLUSH_VCD_CACHE, 1).done());
    w.pkt(Pkt::new(P_START_TILE_BINNING, 1).done());

    // Draw state: VCM cache size (1 batch each for bin+render), the shader-state pointer, then the prim.
    w.pkt(
        Pkt::new(P_VCM_CACHE_SIZE, 2)
            .f(0, 4, 1) // 16-vertex batches for binning
            .f(4, 4, 1) // 16-vertex batches for rendering
            .done(),
    );
    // GL_SHADER_STATE: address is a 27-bit field @ start5 → the record's 32-byte-aligned address's top
    // 27 bits; number of attribute arrays in the low 5 bits.
    w.pkt(
        Pkt::new(P_GL_SHADER_STATE, 4)
            .f(0, 5, num_attrs as u64) // number of attribute arrays
            .f(5, 27, (shadrec >> 5) as u64) // record address (32-byte aligned)
            .done(),
    );
    // VERTEX_ARRAY_PRIMS: draw 3 vertices as a triangle list. mode@0(8)=TRIANGLES(4), length@8(32)=3,
    // index of first vertex@40(32)=0.
    w.pkt(
        Pkt::new(P_VERTEX_ARRAY_PRIMS, 10)
            .f(0, 8, V3D_PRIM_TRIANGLES)
            .f(8, 32, 3) // Length (vertex count)
            .f(40, 32, 0) // Index of First Vertex
            .done(),
    );
    // FLUSH terminates the binning list (the binner-done marker CT0 walks to).
    w.pkt(Pkt::new(P_FLUSH, 1).done());
    w.len()
}

/// Build the M4 RENDER control list (CT1) at OFF_M4_RCL + its generic per-tile sub-list at
/// OFF_M4_SUBLIST. Mirrors the M3 RCL but (a) targets OFF_M4_TARGET, and (b) the sub-list runs
/// BRANCH_TO_IMPLICIT_TILE_LIST so the render EXECUTES the binner's per-tile geometry list (the M3
/// clear-only list omitted this branch — here it is the whole point). Returns `(main_len, sublist_len)`.
fn build_m4_rcl() -> (usize, usize) {
    let target = (arena_phys() + OFF_M4_TARGET) as u32;
    let sublist_start = (arena_phys() + OFF_M4_SUBLIST) as u32;
    let tile_alloc = (arena_phys() + OFF_BIN_TILEALLOC) as u32;
    let stride = (TARGET_W * TARGET_BPP) as u64;

    // ── Generic per-tile sub-list: run the implicit (binned) tile list, then store the tile buffer. ──
    let mut s = RclWriter::new(OFF_M4_SUBLIST);
    s.pkt(Pkt::new(P_TILE_COORDINATES_IMPLICIT, 1).done());
    s.pkt(Pkt::new(P_END_OF_LOADS, 1).done());
    s.pkt(Pkt::new(P_PRIM_LIST_FORMAT, 2).f(0, 6, PRIM_TYPE_LIST_TRIANGLES).done());
    // THE new branch: execute the binner's per-tile primitive list for this tile (set number 0). This is
    // what draws the triangle the binner produced — the M3 clear-job had no geometry so omitted it.
    s.pkt(Pkt::new(P_BRANCH_TO_IMPLICIT_TILE_LIST, 2).f(0, 8, 0).done());
    // Store RT0 → OFF_M4_TARGET, raster, rgba8, row stride (the write the CPU sample-verifies).
    s.pkt(
        Pkt::new(P_STORE_TILE_BUFFER_GENERAL, 13)
            .f(0, 4, 0) // Buffer to Store = Render target 0
            .f(4, 3, MEMORY_FORMAT_RASTER)
            .f(12, 6, OUTPUT_IMAGE_FORMAT_RGBA8)
            .f(28, 20, stride)
            .f(64, 32, target as u64)
            .done(),
    );
    s.pkt(Pkt::new(P_CLEAR_TILE_BUFFERS, 2).f(0, 1, 1).f(1, 1, 1).done());
    s.pkt(Pkt::new(P_END_OF_TILE_MARKER, 1).done());
    s.pkt(Pkt::new(P_RETURN_FROM_SUB_LIST, 1).done());
    let sublist_len = s.len();
    let sublist_end = sublist_start + sublist_len as u32;
    cache::clean_range(arena_phys() + OFF_M4_SUBLIST, sublist_len);

    // ── Main render list: frame config (clear colour = CLEAR_RGBA so OUTSIDE the triangle reads clear),
    // then the single-supertile render that branches into the sub-list. Same structure as M3. ──
    let mut w = RclWriter::new(OFF_M4_RCL);
    w.pkt(
        Pkt::new(P_TRMC, 9)
            .f(0, 4, TRMC_SUBID_COMMON)
            .f(4, 4, 0)
            .f(8, 16, TARGET_W as u64)
            .f(24, 16, TARGET_H as u64)
            .f(40, 2, INTERNAL_BPP_32)
            .done(),
    );
    w.pkt(
        Pkt::new(P_TRMC, 9)
            .f(0, 4, TRMC_SUBID_CLEAR_COLORS_PART1)
            .f(4, 4, 0)
            .f(8, 32, CLEAR_RGBA as u64)
            .done(),
    );
    w.pkt(
        Pkt::new(P_TRMC, 9)
            .f(0, 4, TRMC_SUBID_COLOR)
            .f(4, 2, INTERNAL_BPP_32)
            .f(6, 4, INTERNAL_TYPE_8)
            .f(10, 2, 0)
            .done(),
    );
    w.pkt(
        Pkt::new(P_TRMC, 9)
            .f(0, 4, TRMC_SUBID_ZS_CLEAR_VALUES)
            .f(8, 8, 0)
            .f(16, 32, 0)
            .done(),
    );
    w.pkt(
        Pkt::new(P_TILE_LIST_INITIAL_BLOCK_SIZE, 2)
            .f(0, 2, TILE_ALLOC_BLOCK_SIZE_64B)
            .f(2, 1, 1)
            .done(),
    );
    w.pkt(
        Pkt::new(P_MULTICORE_TILE_LIST_BASE, 5)
            .f(0, 4, 0)
            .f(6, 26, (tile_alloc >> 6) as u64)
            .done(),
    );
    w.pkt(
        Pkt::new(P_MULTICORE_SUPERTILE_CFG, 9)
            .f(0, 8, 0)
            .f(8, 8, 0)
            .f(16, 8, 1)
            .f(24, 8, 1)
            .f(32, 12, 1)
            .f(44, 12, 1)
            .f(61, 3, 0)
            .done(),
    );
    // Initial tile-buffer clear (GFXH-1742 double-dummy-store workaround), same as M3.
    w.pkt(Pkt::new(P_TILE_COORDINATES, 4).f(0, 12, 0).f(12, 12, 0).done());
    for i in 0..2 {
        if i > 0 {
            w.pkt(Pkt::new(P_TILE_COORDINATES, 4).f(0, 12, 0).f(12, 12, 0).done());
        }
        w.pkt(Pkt::new(P_END_OF_LOADS, 1).done());
        w.pkt(Pkt::new(P_STORE_TILE_BUFFER_GENERAL, 13).f(0, 4, 8).done());
        if i == 0 {
            w.pkt(Pkt::new(P_CLEAR_TILE_BUFFERS, 2).f(0, 1, 1).f(1, 1, 1).done());
        }
        w.pkt(Pkt::new(P_END_OF_TILE_MARKER, 1).done());
    }
    w.pkt(Pkt::new(P_FLUSH_VCD_CACHE, 1).done());
    w.pkt(
        Pkt::new(P_GENERIC_TILE_LIST, 9)
            .f(0, 32, sublist_start as u64)
            .f(32, 32, sublist_end as u64)
            .done(),
    );
    w.pkt(Pkt::new(P_SUPERTILE_COORDINATES, 3).f(0, 8, 0).f(8, 8, 0).done());
    w.pkt(Pkt::new(P_END_OF_RENDERING, 1).done());
    (w.len(), sublist_len)
}

/// The CT0 (binning) run/never-ran discriminator — the PI-V3D-7 idiom extended to CT0. Given the
/// pre/kicked/done CS+CA snapshots and the [BA,EA) queue range, classify whether the BIN CLE actually
/// started. Same truth table as the CT1 render discriminator: RAN iff CTRUN was ever observed OR CT0CA
/// advanced INTO (BA, EA]; a never-started CLE has CTRUN never seen AND CT0CA at 0/BA.
fn ct0_ran(cs_pre: u32, cs_kicked: u32, cs_done: u32, ca_done: u32, ba: u32, ea: u32) -> bool {
    let ctrun_ever = (cs_pre | cs_kicked | cs_done) & V3D_CLE_CT1CS_CTRUN != 0; // CTRUN bit is shared
    let ca_advanced = ca_done != 0 && ca_done != ba && ca_done >= ba && ca_done <= ea;
    ctrun_ever || ca_advanced
}

/// M4: bin one triangle on CT0, render it on CT1 (implicit tile list), CPU sample-verify.
/// ATTENDED-METAL-UNVERIFIED — QEMU never reaches here. On success prints the M4 PASS witness + a
/// sample table; the QPU shader body is the one metal-refinement seam (see the module banner).
fn triangle_job(fb: Option<FbTarget>) {
    serial_println!(":: V3D: M4 triangle — binning on CT0, render on CT1 (implicit tile list) ::");

    // (0) Publish the shader programs, vertex data, default attributes. The shaders are the field-
    // validated NOP skeleton (metal-refinement seam); vertex data is the real triangle.
    write_shader_stub(OFF_CS_CODE, 16);
    write_shader_stub(OFF_VS_CODE, 16);
    write_shader_stub(OFF_FS_CODE, 16);
    for (i, v) in TRI_VERTS.iter().enumerate() {
        for (j, comp) in v.iter().enumerate() {
            arena_write_u32(OFF_VTXDATA + i * 16 + j * 4, comp.to_bits());
        }
    }
    fill_region(OFF_DEFAULT_ATTRS, 16, 0); // zeroed default attribute values

    // (1) Build the shader record + attribute record, the binning CL, and the render CL.
    let num_attrs = build_shader_record();
    let bin_len = build_bin_cl(num_attrs);
    let (rcl_len, sublist_len) = build_m4_rcl();

    // (2) Pre-seed the M4 target with a sentinel distinct from BOTH colours, so the sample-verify proves
    // the GPU wrote every pixel it claims (neither clear nor triangle can appear by luck).
    fill_region(OFF_M4_TARGET, TARGET_BYTES, 0x5555_5555);

    // (3) Publish everything to RAM for the non-coherent GPU (shaders, verts, record, both lists, target,
    // and the tile-state / tile-alloc scratch the binner writes and the render reads).
    cache::clean_range(arena_phys() + OFF_CS_CODE, 16 * 8);
    cache::clean_range(arena_phys() + OFF_VS_CODE, 16 * 8);
    cache::clean_range(arena_phys() + OFF_FS_CODE, 16 * 8);
    cache::clean_range(arena_phys() + OFF_VTXDATA, TRI_VERTS.len() * 16);
    cache::clean_range(arena_phys() + OFF_DEFAULT_ATTRS, 16);
    cache::clean_range(arena_phys() + OFF_SHADREC, 36 + 16);
    cache::clean_range(arena_phys() + OFF_BIN_CL, bin_len);
    cache::clean_range(arena_phys() + OFF_M4_RCL, rcl_len);
    cache::clean_range(arena_phys() + OFF_M4_TARGET, TARGET_BYTES);
    let _ = sublist_len; // published inside build_m4_rcl
    fill_region(OFF_TILESTATE, TILE_STATE_BYTES, 0);
    fill_region(OFF_BIN_TILEALLOC, BIN_TILEALLOC_BYTES, 0);
    cache::clean_range(arena_phys() + OFF_TILESTATE, TILE_STATE_BYTES);
    cache::clean_range(arena_phys() + OFF_BIN_TILEALLOC, BIN_TILEALLOC_BYTES);

    // (4) Kick CT0 (the BIN queue). Program the tile-state array (CT0QMA/QMS) first — the binner writes
    // its per-tile primitive lists into the tile-alloc memory keyed by this state — then CT0QBA (begin)
    // and CT0QEA (GO). All addresses are arena-internal identity iovas, bounds-checked (memory-safety).
    let bin_ba = (arena_phys() + OFF_BIN_CL) as u32;
    let bin_ea = bin_ba + bin_len as u32;
    let ts = (arena_phys() + OFF_TILESTATE) as u32;
    if !arena_contains(bin_ba as usize, bin_len) || !arena_contains(ts as usize, TILE_STATE_BYTES) {
        serial_println!(":: V3D: M4 bin range escapes the arena — refusing kick (fail-closed) ::");
        return;
    }
    let ct0_cs_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    let ct0_ca_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CA);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QMA, ts);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QMS, TILE_STATE_BYTES as u32);
    dsb();
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QBA, bin_ba);
    dsb(); // BA latched before the GO
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QEA, bin_ea); // GO
    dsb();
    let ct0_cs_kicked = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    let ct0_ca_kicked = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CA);
    let bin_idled = wait_bit_clear(V3D_CORE0_BASE, V3D_CLE_CT0CS, V3D_CLE_CT1CS_CTRUN, "CT0 bin");
    let ct0_cs_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    let ct0_ca_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CA);
    let mmu_ctl_bin = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let bin_fault = mmu_ctl_bin
        & (V3D_MMU_CTL_PT_INVALID | V3D_MMU_CTL_WRITE_VIOLATION | V3D_MMU_CTL_CAP_EXCEEDED);
    let bin_ran = ct0_ran(ct0_cs_pre, ct0_cs_kicked, ct0_cs_done, ct0_ca_done, bin_ba, bin_ea);
    serial_println!(
        ":: V3D: M4 bin clue — CT0CS pre={:#010x} kicked={:#010x} done={:#010x} CT0CA pre={:#010x} kicked={:#010x} done={:#010x} (BA={:#010x} EA={:#010x}) ran={} idled={} MMU_fault={:#x} ::",
        ct0_cs_pre, ct0_cs_kicked, ct0_cs_done, ct0_ca_pre, ct0_ca_kicked, ct0_ca_done,
        bin_ba, bin_ea, bin_ran as u32, bin_idled as u32, bin_fault
    );
    super::exceptions::serror_drain_request("v3d: M4 bin kick window");

    // (5) Kick CT1 (the RENDER queue) over the M4 RCL — same submit path as M3, different list. It
    // consumes the binner's per-tile lists via BRANCH_TO_IMPLICIT_TILE_LIST.
    let rcl_ba = (arena_phys() + OFF_M4_RCL) as u32;
    let rcl_ea = rcl_ba + rcl_len as u32;
    if !arena_contains(rcl_ba as usize, rcl_len) {
        serial_println!(":: V3D: M4 render range escapes the arena — refusing kick (fail-closed) ::");
        return;
    }
    let r_cs_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CS);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT1QBA, rcl_ba);
    dsb();
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT1QEA, rcl_ea); // GO
    dsb();
    let r_cs_kicked = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CS);
    let r_idled = wait_bit_clear(V3D_CORE0_BASE, V3D_CLE_CT1CS, V3D_CLE_CT1CS_CTRUN, "CT1 M4 render");
    let r_cs_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CS);
    let r_ca_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CA);
    let mmu_ctl_r = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let r_fault = mmu_ctl_r
        & (V3D_MMU_CTL_PT_INVALID | V3D_MMU_CTL_WRITE_VIOLATION | V3D_MMU_CTL_CAP_EXCEEDED);
    let r_ran = ct0_ran(r_cs_pre, r_cs_kicked, r_cs_done, r_ca_done, rcl_ba, rcl_ea);
    serial_println!(
        ":: V3D: M4 render clue — CT1CS pre={:#010x} kicked={:#010x} done={:#010x} CT1CA done={:#010x} (BA={:#010x} EA={:#010x}) ran={} idled={} MMU_fault={:#x} ::",
        r_cs_pre, r_cs_kicked, r_cs_done, r_ca_done, rcl_ba, rcl_ea, r_ran as u32, r_idled as u32, r_fault
    );
    super::exceptions::serror_drain_request("v3d: M4 render kick window");

    if !bin_idled || !r_idled {
        serial_println!(":: V3D: M4 — a CLE did not idle within budget (anti-hang backstop) — no verify ::");
        return;
    }

    // (6) CPU sample-verify: pull the target back from DRAM and check inside/outside samples.
    cache::clean_invalidate_range(arena_phys() + OFF_M4_TARGET, TARGET_BYTES);
    let pass = verify_triangle_samples();
    if pass {
        serial_println!(":: V3D: M4 triangle — PASS (inside samples = triangle colour, outside = clear) ::");
        if let Some(fb) = fb {
            blit_m4_target(&fb);
        }
    } else {
        serial_println!(":: V3D: M4 triangle — FAIL/UNRENDERED (see sample table; QPU shader body is the metal-refinement seam) ::");
    }
}

/// Read one 32-bit pixel from the M4 target at (x, y).
#[inline]
fn m4_sample(x: usize, y: usize) -> u32 {
    let off = OFF_M4_TARGET + (y * TARGET_W + x) * TARGET_BPP;
    let arena = &raw const V3D_ARENA;
    unsafe {
        let b = &(*arena).bytes;
        u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
    }
}

/// Sample-verify the rendered triangle: ≥3 interior points must equal TRI_RGBA and ≥3 exterior points
/// must equal CLEAR_RGBA (per the brief). Interior points cluster around the centroid (~32,32); exterior
/// points sit in the corners the centred triangle does not cover. Prints the full sample table (the M4
/// witness) so the attended sitting sees exactly what landed even on a partial render.
fn verify_triangle_samples() -> bool {
    // Screen coords chosen from TRI_VERTS mapped to the 64×64 viewport (y-down): centroid ≈ (32,34).
    let inside: [(usize, usize); 3] = [(32, 34), (26, 40), (38, 40)];
    let outside: [(usize, usize); 3] = [(2, 2), (61, 2), (32, 4)];
    let mut ok = true;
    for (x, y) in inside {
        let px = m4_sample(x, y);
        let hit = px == TRI_RGBA;
        ok &= hit;
        serial_println!(
            ":: V3D: M4 sample IN  ({:2},{:2}) = {:#010x} expect {:#010x} {} ::",
            x, y, px, TRI_RGBA, if hit { "OK" } else { "MISS" }
        );
    }
    for (x, y) in outside {
        let px = m4_sample(x, y);
        let hit = px == CLEAR_RGBA;
        ok &= hit;
        serial_println!(
            ":: V3D: M4 sample OUT ({:2},{:2}) = {:#010x} expect {:#010x} {} ::",
            x, y, px, CLEAR_RGBA, if hit { "OK" } else { "MISS" }
        );
    }
    ok
}

/// Blit the M4 target next to the M3 target on the panel (metal visible witness) — offset to the right
/// so both are visible. Bounds-clipped to the framebuffer.
fn blit_m4_target(fb: &FbTarget) {
    if fb.base == 0 || fb.bytes_per_pixel < 4 {
        return;
    }
    let x_origin = TARGET_W + 8; // to the right of the M3 blit
    let w = TARGET_W.min(fb.width.saturating_sub(x_origin));
    let h = TARGET_H.min(fb.height);
    for y in 0..h {
        for x in 0..w {
            let px = m4_sample(x, y);
            let dst = fb.base as usize
                + y * fb.stride_px * fb.bytes_per_pixel
                + (x_origin + x) * fb.bytes_per_pixel;
            if dst + 4 <= fb.base as usize + fb.size {
                unsafe { core::ptr::write_volatile(dst as *mut u32, px) };
            }
        }
    }
}
