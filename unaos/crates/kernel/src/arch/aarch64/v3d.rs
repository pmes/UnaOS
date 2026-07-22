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
use core::sync::atomic::{AtomicBool, Ordering};

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

// ─── PI-V3D-12: GPU-side cache maintenance (core 0). Offsets + bits transcribed VERBATIM from Linux
// `drivers/gpu/drm/v3d/v3d_regs.h` (register/hardware facts; GPL-2.0-only header, facts-only — same
// discipline as the CT-queue and MMU constants above). Linux `v3d_gem.c::v3d_invalidate_caches` runs
// before EVERY job (both `v3d_bin_job_run` and `v3d_render_job_run` call it, per v3d_sched.c): on
// V3D >= 4.1 the live steps are the L2T flush (L2TCACTL: L2TFLS with FLM=FLUSH) and the slice-cache
// invalidate (SLCACTL: all-0xF TVCCS/TDCCS/UCC/ICC); the GCA/L3 step is ver<41-only and the L2C
// invalidate ver<33-only — both no-ops on the Pi 4's 4.2, so neither is transcribed here.
const V3D_CTL_SLCACTL: usize = 0x0024; // slice-cache control (TMU-vertex/TMU-data/uniform/instruction)
const V3D_CTL_L2TCACTL: usize = 0x0030; // L2T cache control
const V3D_L2TCACTL_L2TFLS: u32 = 1 << 0; // flush start; reads 1 while the flush is in progress
const V3D_L2TCACTL_FLM_FLUSH: u32 = 0 << 1; // FLM field [2:1] = FLUSH (write-back + invalidate)
const V3D_SLCACTL_INVALIDATE_ALL: u32 = (0xF << 24) | (0xF << 16) | (0xF << 8) | 0xF;

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
const V3D_CLE_CT0QMA: usize = 0x0170; // CT0 bin TILE-ALLOCATION memory base (v3d_regs.h V3D_CLE_CT0QMA)
const V3D_CLE_CT0QMS: usize = 0x0174; // CT0 bin TILE-ALLOCATION memory size  (v3d_regs.h V3D_CLE_CT0QMS)
// PI-V3D-9 boot-P5 root cause: the M4 base wrote the tile-STATE region (192 B) into CT0QMA/QMS as if it
// were the tile-ALLOCATION pool and never programmed CT0QTS at all. Per Linux v3d_sched.c
// `v3d_bin_job_run` the three are DISTINCT: CT0QMA/QMS = tile-ALLOCATION memory (the pool the binner
// grows per-tile primitive lists into), CT0QTS = tile-STATE data array (ENABLE-gated). Handing the
// binner a 192-byte "pool" overflowed it immediately → it walked off into an unmapped page →
// PT_INVALID (MMU_fault bit20) with CT0CA halted mid-list. Corrected below. CT0QTS offset + ENABLE bit
// transcribed VERBATIM from Linux v3d_regs.h (register facts; GPL-2.0-only header, facts-only).
const V3D_CLE_CT0QTS: usize = 0x015c; // CT0 bin tile-STATE data array base (v3d_regs.h V3D_CLE_CT0QTS)
const V3D_CLE_CT0QTS_ENABLE: u32 = 1 << 1; // v3d_regs.h V3D_CLE_CT0QTS_ENABLE — gate the tile-state write
// PI-V3D-13 fact-check (Linux v3d_regs.h + v3d_sched.c v3d_bin_job_run, verbatim; facts only —
// GPLv2): CT0QTS=0x15c with ENABLE=BIT(1), CT0QBA=0x160, CT0QEA=0x168, CT0QMA=0x170, CT0QMS=0x174;
// bin submit order = invalidate caches, then CT0QMA (pool base) → CT0QMS (pool SIZE, not end) →
// CT0QTS|ENABLE → CT0QBA → CT0QEA (GO). On 4.x the TILE_BINNING_MODE_CFG packet carries only the
// tile-alloc BLOCK-SIZE enums (Mesa v3dvx_cmd_buffer.c job_emit_binning_prolog), never the pool
// address — the pool/state addresses travel ONLY through these registers. This file's programming
// already matches all of it (PI-V3D-9); PI-V3D-13 adds the pre-kick readback + post-bin pool-head
// witnesses so the next metal sitting sees exactly which half of that story the silicon disputes.
// CTnCS status: only CTRUN (bit5) is corroborated across sources for V3D 4.x; the remaining bits differ
// from the VideoCore-IV layout and are reported raw rather than guessed (no fabricated bit names).
const V3D_CLE_CTNCS_CTRUN: u32 = 1 << 5;

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
// PI-V3D-9 uniform streams (each a bounded slice of one page, all inside the identity-mapped arena).
// The FS/VS/CS shader-record uniforms-address fields point here; the QPU pops these in FIFO order via
// the ldunifrf signal (and, for the FS, the TLBU-config pops).
const OFF_FS_UNIF: usize = 0x20000; // fragment-shader uniform stream (colour channels + TLB configs)
const OFF_CS_UNIF: usize = 0x20040; // coordinate-shader uniform stream (VPM read offsets)
const OFF_VS_UNIF: usize = 0x20080; // vertex-shader uniform stream (VPM read offsets)
// PI-V3D-14 pool sizing per Mesa v3d_util.c v3d_tile_alloc_sizes (the config the PTB is validated
// against): tiles_size = layers × tiles_x × tiles_y × 128 (INITIAL block, STATIC_ASSERTed == 128);
// pool = align(tiles_size, 4096) + 8192 ("the HW won't trigger OOM during the first allocations")
// + a draw-scaled continuation slush, page-aligned. Our 64×64 fb = 1 layer × 1×1 tiles →
// tiles_size = 128 → align 4096 + 8192 = 12,288 → page-aligned 16 KiB with slush. The existing
// 32 KiB region already covers Mesa's minimum with 2× headroom — kept (no arena-layout change).
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
    let m4_pass = triangle_job(fb);

    // ── PI-V3D-11: the visible graphics battery (M5 gradient → M6 animated → M7 multi-primitive →
    // M8 blit-to-scanout). Purely ADDITIVE stages layered on the M4 scaffold: M3 + M4 above remain
    // the regression witnesses and none of their buffers or kick code is touched. PI-V3D-12: gated
    // on the M4 verdict — the ONLY battery gate. The stages reuse the M4 shaders/scaffold, so on an
    // M4 FAIL they can only bury the M4 witness in derivative noise; the boot the triangle lands,
    // the battery runs. ─────────────────────────────────────────────────────────────────────────
    if m4_pass {
        battery(fb);
        // PI-APP-1: the block is up, the MMU is programmed, and the visible battery just ran to
        // completion off `fb`. Latch that state so the `v3d` shell app can REPLAY the visible stages
        // on the live framebuffer while the system is up (the boot flash is too fast for the monitor
        // to catch). Replay reuses THIS initialized state — power/clock/PM-ASB/MMU stay enabled from
        // boot; only the per-stage jobs (which rebuild their own arena control lists idempotently) are
        // re-kicked. We store the exact FbTarget the boot battery used so replay is byte-for-byte the
        // same path, and do NOT re-enter `bringup` (which would re-power/re-clock/re-program the MMU).
        unsafe {
            V3D_REPLAY_FB = fb;
        }
        V3D_REPLAY_READY.store(true, Ordering::Release);
    } else {
        serial_println!(":: V3D: PI-V3D-11 battery SKIPPED — gated on the M4 triangle verdict (FAIL this boot) ::");
    }

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
// PI-V3D-10 boot-P6 root cause #2 (the render-kick gate): this constant was 0 — the "Halt" opcode —
// mislabeled as Mesa's END_OF_RENDERING. In v3d_packet.xml they are DISTINCT packets: code 0 = Halt,
// code 13 = "End of rendering" (shortname end_render), and BOTH v3dx_rcl.c (gallium) and
// v3dvx_cmd_buffer.c (v3dv) terminate every RCL with END_OF_RENDERING, never Halt. The difference is
// load-bearing for the QUEUED kick path (CTnQBA/QEA): END_OF_RENDERING completes the FRAME (the CLE
// returns to idle and the next queued CT1 job may dispatch), while Halt merely stops the CLE with the
// frame still open. M3's Halt-terminated list therefore "passed" (its store had already landed) but
// left CT1 wedged in the halted frame — the exact boot-P6 signature: M4's CT1QEA write was accepted,
// CTRUN never set, CT1CA parked at M3's end (0x001f806a). Same fabricated-value class as PI-V3D-4/-7.
const P_END_OF_RENDERING: u8 = 13; // "End of rendering" (v3d_packet.xml code 13) — NOT Halt (0)

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
// Tile-allocation block-size enum (v3d_packet.xml, shared by TILE_BINNING_MODE_CFG and
// TILE_LIST_INITIAL_BLOCK_SIZE): 64b = 0, 128b = 1, 256b = 2. PI-V3D-14: Mesa's ONLY exercised
// config on silicon is 128B initial + 64B overflow — v3d_limits.h defines
// V3D_TILE_ALLOC_INITIAL_BLOCK_SIZE 128 / V3D_TILE_ALLOC_OVERFLOW_BLOCK_SIZE 64 with
// enum = (size >> 7), and v3d_util.c STATIC_ASSERTs the initial size == 128. Both emitters
// (v3dvx_cmd_buffer.c job_emit_binning_prolog + cmd_buffer_render_pass_setup_render_pass_rcl,
// v3dx_draw.c/v3dx_rcl.c on the GL side) use INITIAL(=1) in the bin config's initial-block field
// AND in the render list's TILE_LIST_INITIAL_BLOCK_SIZE ("needs to match the value from binning
// mode config"), and OVERFLOW(=0) only in the bin config's (overflow) block-size field.
const TILE_ALLOC_BLOCK_SIZE_64B: u64 = 0;
const TILE_ALLOC_BLOCK_SIZE_128B: u64 = 1;

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
    // TILE_LIST_INITIAL_BLOCK_SIZE — must precede the first branch; auto-chained, 128-byte first
    // block (PI-V3D-14: must MATCH the bin config's initial-block-size — Mesa fact).
    w.pkt(
        Pkt::new(P_TILE_LIST_INITIAL_BLOCK_SIZE, 2)
            .f(0, 2, TILE_ALLOC_BLOCK_SIZE_128B) // Size of first block
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

/// PI-V3D-10: decode the V3D MMU violation witness pair into (client name, true VA). Hardware facts
/// from Linux drm/v3d (facts-only, no code lifted): the violating AXI client is VIO_ID >> 5 indexing
/// {L2T, PTB, PSE, TLB, CLE, TFU, MMU, GMP} on V3D 4.1+ (v3d_irq.c), and VIO_ADDR holds the VA
/// right-shifted by (va_width − 32), where va_width = 30 + DEBUG_INFO[7:4] (v3d_drv.c). Boot-P6
/// ground truth: DEBUG_INFO 0x550 → va_width 35 → shift 3; VIO_ADDR 0x04841800 → VA 0x2420C000.
fn vio_decode(vio_id: u32, vio_addr: u32) -> (&'static str, u64) {
    const CLIENTS: [&str; 8] = ["L2T", "PTB", "PSE", "TLB", "CLE", "TFU", "MMU", "GMP"];
    let client = CLIENTS[((vio_id >> 5) & 0x7) as usize];
    let dbg = mmio_read(V3D_HUB_BASE, V3D_MMU_DEBUG_INFO);
    let va_width = 30 + ((dbg >> 4) & 0xF) as u64;
    let shift = va_width.saturating_sub(32);
    (client, (vio_addr as u64) << shift)
}

/// PI-V3D-9: clear any latched V3D-MMU translation fault (PT_INVALID / WRITE_VIOLATION / CAP_EXCEEDED),
/// mirroring Linux `v3d_irq.c`: read V3D_MMU_CTL and write it straight back — the fault-status bits are
/// write-1-to-clear, so echoing the read value clears the latched fault while the ENABLE/abort config
/// bits (also echoed) are preserved. Reports whether a fault was actually latched (the witness the
/// attended sitting reads to correlate a render-kick refusal with a sticky bin fault). Reads-then-one-
/// write; cannot fault or hang (QEMU-safe — the CTL reads 0/absent there and the write-back is inert).
fn clear_mmu_fault_latch(when: &str) {
    let ctl = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let fault = ctl & (V3D_MMU_CTL_PT_INVALID | V3D_MMU_CTL_WRITE_VIOLATION | V3D_MMU_CTL_CAP_EXCEEDED);
    if fault != 0 {
        let vio_addr = mmio_read(V3D_HUB_BASE, V3D_MMU_VIO_ADDR);
        let vio_id = mmio_read(V3D_HUB_BASE, V3D_MMU_VIO_ID);
        mmio_write(V3D_HUB_BASE, V3D_MMU_CTL, ctl); // W1C: echo clears the sticky fault bits
        dsb();
        let after = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
        let (client, va) = vio_decode(vio_id, vio_addr);
        serial_println!(
            ":: V3D: MMU fault-latch CLEARED ({}) — was CTL={:#010x} (PT_INVALID={} WRITE_VIOLATION={} CAP_EXCEEDED={}) VIO_ADDR={:#010x} VIO_ID={:#010x} (client {} @ VA {:#010x}) -> CTL={:#010x} ::",
            when, ctl,
            (fault & V3D_MMU_CTL_PT_INVALID != 0) as u32,
            (fault & V3D_MMU_CTL_WRITE_VIOLATION != 0) as u32,
            (fault & V3D_MMU_CTL_CAP_EXCEEDED != 0) as u32,
            vio_addr, vio_id, client, va, after
        );
    } else {
        serial_println!(":: V3D: MMU fault-latch clear ({}) — none latched (CTL={:#010x}) ::", when, ctl);
    }
}

/// PI-V3D-12: the pre-kick GPU-cache invalidate — the Linux `v3d_invalidate_caches` idiom every job
/// submission runs (v3d_sched.c calls it in BOTH `v3d_bin_job_run` and `v3d_render_job_run`). On the
/// Pi 4's V3D 4.2 the two live steps are:
///   (1) L2T flush (L2TCACTL <= L2TFLS | FLM=FLUSH): write back + invalidate the L2T — this is what
///       PUBLISHES a prior GPU engine's memory writes (the PTB's binned tile lists) to the next
///       engine's fetch path, and drops any stale line caching the CPU's pre-job contents;
///   (2) slice-cache invalidate (SLCACTL <= all-0xF): drop the per-slice TMU/uniform/instruction
///       caches so shaders fetch current code/uniforms.
/// The L2TFLS wait is the standard finite backstop (Linux waits on the same bit in v3d_clean_caches);
/// a timeout is logged and the caller proceeds — the kick's own witnesses stay decisive. Boot-P7 root
/// cause (PI-V3D-12): our driver never did ANY of this. M3 survived because its CT1 only ever read
/// CPU-published lists (CPU-side cache cleans cover CPU→GPU); M4's render is the FIRST job whose CLE
/// must observe ANOTHER GPU job's output (the bin's tile lists) — with the L2T never flushed, the
/// BRANCH_TO_IMPLICIT_TILE_LIST fetch at the tile-alloc base returned the stale pre-bin zero-fill,
/// opcode 0x00 = Halt, and the CLE stopped inside the pool (CT1CA parked BELOW BA) before ever
/// reaching the sub-list's STORE — render "clean", zero stores.
fn invalidate_gpu_caches(what: &str) {
    mmio_write(V3D_CORE0_BASE, V3D_CTL_L2TCACTL, V3D_L2TCACTL_L2TFLS | V3D_L2TCACTL_FLM_FLUSH);
    let _ = wait_bit_clear(V3D_CORE0_BASE, V3D_CTL_L2TCACTL, V3D_L2TCACTL_L2TFLS, what);
    mmio_write(V3D_CORE0_BASE, V3D_CTL_SLCACTL, V3D_SLCACTL_INVALIDATE_ALL);
    dsb();
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
// offsets — twice), PI-V3D-9 generates every shader word with Mesa's OWN packer
// (`v3d_qpu_instr_pack`, ver=42) from explicit instruction structs, round-trips each through Mesa's
// unpacker, and cross-checks the generator against four canonical `qpu_disasm.c` vectors bit-exactly
// (see the "VERIFIED QPU shader bodies" block below for the full provenance + the honest split between
// Mesa-verified ENCODING and the silicon-tuned geometry/colour quantities that remain the attended-
// metal-refinement surface). `triangle_job` witnesses the CT0 bin discriminator and the CT1 render
// regardless, and the CPU sample-verify reports exactly which samples matched, so the sitting is
// decisive.

// ─── Binning-side + shared packet opcodes (v3d_packet.xml `code=`). ───
const P_FLUSH: u8 = 4; // Flush — terminates the binning list (binner-done signal)
const P_START_TILE_BINNING: u8 = 6; // must follow the bin-mode config before geometry
const P_BRANCH_TO_IMPLICIT_TILE_LIST: u8 = 21; // render: run the binner's per-tile list for this tile
const P_VERTEX_ARRAY_PRIMS: u8 = 36; // non-indexed draw
const P_GL_SHADER_STATE: u8 = 64; // points at the GL Shader State Record + attribute records
const P_VCM_CACHE_SIZE: u8 = 71;
const P_NUMBER_OF_LAYERS: u8 = 119;
const P_TILE_BINNING_MODE_CFG: u8 = 120; // v42 variant (max_ver=42)
// PI-V3D-17 — clip/viewport/config state (v3d_packet.xml, gen 4.2). Codes transcribed VERBATIM:
//   Cfg Bits code=96 (max_ver=42), clip_window code=107, Viewport Offset code=108,
//   Clipper XY Scaling code=110 (max_ver=42), Clipper Z Scale and Offset code=111.
const P_CFG_BITS: u8 = 96; // "Cfg Bits" (max_ver=42) — facing/cull + rasterizer config
const P_CLIP_WINDOW: u8 = 107; // "clip_window" — scissor/clip rect in pixels
const P_VIEWPORT_OFFSET: u8 = 108; // "Viewport Offset" — screen-space centre (coarse int + fine u14.8)
const P_CLIPPER_XY_SCALING: u8 = 110; // "Clipper XY Scaling" (max_ver=42) — half w/h in 1/256 px, f32
const P_CLIPPER_Z_SCALE_AND_OFFSET: u8 = 111; // "Clipper Z Scale and Offset" — z scale/offset, f32

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

// ════════════════════════════════════════════════════════════════════════════════════════════════
// PI-V3D-9 — VERIFIED QPU shader bodies (replacing the V3D-8 NOP skeletons).
// ════════════════════════════════════════════════════════════════════════════════════════════════
//
// PROVENANCE (absolute — this driver has been convicted THREE times for fabricated words; not again):
// every 64-bit word below was produced by Mesa's OWN packer, `v3d_qpu_instr_pack(devinfo.ver=42, …)`,
// from an explicit `struct v3d_qpu_instr` — i.e. these ARE Mesa's encoder output, not hand-authored
// bit patterns. The generator (`scratchpad/mesa/qpu_gen.c`, MIT, links Mesa's qpu_instr.c + qpu_pack.c
// from mesa 26.3.0-devel) additionally ROUND-TRIPS each word (pack → v3d_qpu_instr_unpack → repack,
// require identical) and, as a harness self-test, reproduces four canonical vectors from Mesa's
// `src/broadcom/qpu/tests/qpu_disasm.c` BIT-EXACTLY, proving the struct→word path matches Mesa's
// documented disasm semantics:
//     nop                     = 0x3c003186bb800000   (also the file's qpu_nop() self-check)
//     or rf0,r3,r3;mov vpm,r3 = 0x3c002380b6edb000
//     vfpack tlb, r0, r1      = 0x3c00318735808000
//     fadd r1,r1,r5 ; thrsw   = 0x3c20318105829000
// Mesa is MIT-licensed — verbatim-liftable WITH attribution (memory: unaos-license-gplv3).
//
// VERIFICATION LEVEL (honest, per the module's ATTENDED-METAL-UNVERIFIED banner): every WORD's ENCODING
// is Mesa-verified (round-trips against Mesa's own pack/unpack). The PROGRAMS follow Mesa's documented
// emit ORDER for the minimal cases (fragment: emit_frag_end + vir_emit_tlb_color_write; coord/vertex:
// ntq_emit_vpm_read + emit_store_output_vs + emit_vert_end). What remains the attended-metal-refinement
// surface — NOT fabrication, but silicon-tuned quantities QEMU cannot exercise — is: the coordinate
// viewport transform + exact VPM output layout/segment sizes, the VPM read-offset/setup values, and the
// FS colour-channel order + f16 rounding that make the stored word land exactly on TRI_RGBA. Each such
// quantity is called out at its uniform/word below.

/// FRAGMENT shader (solid colour → TLB). Mesa emit_frag_end path: 4× ldunifrf load colour rgba into
/// rf0..rf3, passthrough-Z `mov tlbu` (pops the Z TLB-config uniform), then two VFPACKs pack rgba to
/// f16 and write TLB (the rg write is the `u` variant, popping the colour TLB-config uniform). thrsw +
/// two nops close the (single) thread. Uniform FIFO order: r,g,b,a, Z-config, colour-config.
const FS_WORDS: [u64; 10] = [
    0x3d80_3186_bb80_0000, // nop ; ldunifrf.rf0   (rf0 <- colour.r)
    0x3d80_7186_bb80_0000, // nop ; ldunifrf.rf1   (rf1 <- colour.g)
    0x3d80_b186_bb80_0000, // nop ; ldunifrf.rf2   (rf2 <- colour.b)
    0x3d80_f186_bb80_0000, // nop ; ldunifrf.rf3   (rf3 <- colour.a)
    0x3c00_3206_bbe0_0000, // mov tlbu, r0         (passthrough-Z; pops Z TLB-config)
    0x3c00_3188_3583_e001, // vfpack tlbu, rf0, rf1 (colour r,g → f16; pops colour TLB-config)
    0x3c00_3187_3583_e083, // vfpack tlb, rf2, rf3  (colour b,a → f16)
    0x3c20_3186_bb80_0000, // nop ; thrsw          (last thread switch)
    0x3c00_3186_bb80_0000, // nop
    0x3c00_3186_bb80_0000, // nop
];

/// COORDINATE / VERTEX shader body — the SIX-word screen-space output (same program for the bin CS and
/// render VS variants), written with the V3D 4.2 STVPMV output mechanism.
///
/// PI-V3D-20 ROOT-CAUSE FIX: PI-V3D-9/17/18/19 wrote the VPM output with the *streamed* VC4 / V3D-3.3
/// mechanism — a `vpmsetup` to arm a VPM segment, then `mov vpm, rfN` (magic waddr VPM=14) auto-advancing
/// an implicit write pointer. That mechanism DOES NOT EXIST for per-vertex shader output on V3D 4.x
/// (ver==42, the Pi 4). Mesa proves it: `vir_VPM_WRITE` (src/broadcom/compiler/nir_to_vir.c) emits ONE
/// `vir_STVPMV(c, vir_uniform_ui(c, vpm_index), val)` per output component — a store-VPM with an EXPLICIT
/// integer VPM offset — and NO `mov vpm` / `vpmsetup` anywhere in the ver-42 VS/CS output path. So every
/// prior `mov vpm` (clip words AND the V3D-19 screen words) wrote an unconfigured magic register; the PTB
/// read zero screen coords and binned an empty-but-legal list (metal boot-P19: pool/tile-STATE all zero,
/// CL clean). No word-count change (4→6) ever moved it because the *addressing form* was wrong, not the
/// count. This body switches to STVPMV with explicit per-component offsets (0..5), sourced as uniforms
/// (Mesa-faithful) into rf9..rf14, and DROPS vpmsetup (unused on 4.x VS/CS output). VPMWT stays
/// (GFXH-1684, ver==42 emit_vert_end). `vpmsetup` DOES pack on ver 42 (opcode 187, first_ver 33) but on
/// 4.x it arms VPM *DMA* descriptors, not the shader output stream — an irrelevant channel.
///
/// W=1 SIMPLIFICATION (LOUD): TRI_VERTS all carry Wc = 1.0, so 1/Wc = 1.0 and NO reciprocal (SFU/recip)
/// is emitted — the transform collapses to Xs = f2i32(floor(Xc·8192)) (8192 = vp_scale 32·256). This
/// holds ONLY for W=1 geometry; a perspective draw (W≠1) would need a per-vertex reciprocal here.
///
/// Mesa order: 4× ldvpmv_in read the vec4 clip position into rf0..rf3 (each reloads the read-offset into
/// rf5); ldunifrf loads 8192.0f into rf6 then the six output offsets 0..5 into rf9..rf14; per screen axis
/// fmul→ffloor→ftoiz into rf7/rf8; then 6× `stvpmv rf<off>, rf<val>` store clip[0..3] + screen[4,5];
/// vpmwt (GFXH-1684); thrsw + two nops end. Registers: rf0..3 clip, rf5 in-offset, rf6=8192.0, rf7=Xs,
/// rf8=Ys, rf9..14 out-offsets 0..5.
///
/// PROVENANCE: every word Mesa-packed (v3d_qpu_instr_pack, ver 42) + round-tripped by
/// scripts/pi-v3d20-qpu-gen.c (see its .out.txt). Metal-refinement surface (unchanged stance): the
/// ldunifrf read-offsets and the RF write→read hazard scheduling — QEMU models no V3D, metal decides.
const CS_VS_WORDS: [u64; 27] = [
    0x3d81_6180_bc80_6140, // ldvpmv_in rf0, rf5 ; ldunifrf.rf5   (attr[0] -> Xc)
    0x3d81_6181_bc80_6140, // ldvpmv_in rf1, rf5 ; ldunifrf.rf5   (attr[1] -> Yc)
    0x3d81_6182_bc80_6140, // ldvpmv_in rf2, rf5 ; ldunifrf.rf5   (attr[2] -> Zc)
    0x3d81_6183_bc80_6140, // ldvpmv_in rf3, rf5 ; ldunifrf.rf5   (attr[3] -> Wc)
    0x3d81_b186_bb80_0000, // nop ; ldunifrf.rf6                  (rf6 <- 8192.0f vp_scale)
    0x3d82_7186_bb80_0000, // nop ; ldunifrf.rf9                  (out-offset 0)
    0x3d82_b186_bb80_0000, // nop ; ldunifrf.rf10                 (out-offset 1)
    0x3d82_f186_bb80_0000, // nop ; ldunifrf.rf11                 (out-offset 2)
    0x3d83_3186_bb80_0000, // nop ; ldunifrf.rf12                 (out-offset 3)
    0x3d83_7186_bb80_0000, // nop ; ldunifrf.rf13                 (out-offset 4)
    0x3d83_b186_bb80_0000, // nop ; ldunifrf.rf14                 (out-offset 5)
    0x5400_11c6_bbf8_0006, // fmul rf7, rf0, rf6                  (Xc · 8192.0 ; W=1 so no 1/Wc)
    0x3c00_2187_f680_61c0, // ffloor rf7, rf7                     (floor, ver==42 path)
    0x3c00_2187_f583_e1c0, // ftoiz rf7, rf7                      (f2i32)
    0x5400_1206_bbf8_0046, // fmul rf8, rf1, rf6                  (Yc · 8192.0)
    0x3c00_2188_f680_6200, // ffloor rf8, rf8                     (floor, ver==42 path)
    0x3c00_2188_f583_e200, // ftoiz rf8, rf8                      (f2i32)
    0x3c00_2180_f883_e240, // stvpmv rf9, rf0                     (out0 clip Xc @ offset 0)
    0x3c00_2180_f883_e281, // stvpmv rf10, rf1                    (out1 clip Yc @ offset 1)
    0x3c00_2180_f883_e2c2, // stvpmv rf11, rf2                    (out2 clip Zc @ offset 2)
    0x3c00_2180_f883_e303, // stvpmv rf12, rf3                    (out3 clip Wc @ offset 3)
    0x3c00_2180_f883_e347, // stvpmv rf13, rf7                    (out4 screen Xs @ offset 4)
    0x3c00_2180_f883_e388, // stvpmv rf14, rf8                    (out5 screen Ys @ offset 5)
    0x3c00_3186_bb81_6000, // vpmwt                               (VPM writes complete before end)
    0x3c20_3186_bb80_0000, // nop ; thrsw                         (end)
    0x3c00_3186_bb80_0000, // nop
    0x3c00_3186_bb80_0000, // nop
];

/// Write a table of QPU words (little-endian fetch order) into the arena at `off`. Returns byte length.
fn write_shader_words(off: usize, words: &[u64]) -> usize {
    for (i, w) in words.iter().enumerate() {
        arena_write_u64(off + i * 8, *w);
    }
    words.len() * 8
}

/// The fragment-shader uniform stream (FIFO order matches FS_WORDS' pops). Colour channels are the
/// unorm8 decomposition of TRI_RGBA as f32; the exact channel order + f16 rounding that lands the
/// stored word on TRI_RGBA is the metal-refinement surface. The two TLB config words follow Mesa
/// vir_emit_tlb_color_write: Z = passthrough/per-pixel (0xffffff84), colour = F16 RT0 vec4 per-pixel
/// (0xffffff3f).
fn write_fs_uniforms(off: usize) -> usize {
    let r = ((TRI_RGBA & 0xFF) as f32 / 255.0).to_bits();
    let g = (((TRI_RGBA >> 8) & 0xFF) as f32 / 255.0).to_bits();
    let b = (((TRI_RGBA >> 16) & 0xFF) as f32 / 255.0).to_bits();
    let a = (((TRI_RGBA >> 24) & 0xFF) as f32 / 255.0).to_bits();
    let unif: [u32; 6] = [r, g, b, a, 0xFFFF_FF84, 0xFFFF_FF3F];
    for (i, w) in unif.iter().enumerate() {
        arena_write_u32(off + i * 4, *w);
    }
    unif.len() * 4
}

/// The coord/vertex uniform stream: the four VPM read-offsets (attribute component 0..3) the
/// ldvpmv_in instructions consume via ldunifrf.rf5, then the 8192.0f viewport scale (vp_scale =
/// viewport.scale 32 · clipper_xy_granularity 256) the screen-space `ldunifrf.rf6` consumes to compute
/// Xs/Ys = f2i32(floor(coord · 8192)), then the SIX output VPM offsets 0..5 (PI-V3D-20) that the
/// `ldunifrf.rf9..rf14` load for the STVPMV stores (Mesa sources these as `vir_uniform_ui(c, vpm_index)`).
/// Read-offsets are the metal-refinement surface; 8192.0 is the V3D-18-proven contract constant; the
/// output offsets are the fixed VPM out-slots 0..5 of the 6-word coordinate contract.
fn write_geo_uniforms(off: usize) -> usize {
    let unif: [u32; 11] = [0, 1, 2, 3, 0x4600_0000 /* 8192.0f32 */, 0, 1, 2, 3, 4, 5];
    for (i, w) in unif.iter().enumerate() {
        arena_write_u32(off + i * 4, *w);
    }
    unif.len() * 4
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
    let fs_unif = (arena_phys() + OFF_FS_UNIF) as u64;
    let vs_unif = (arena_phys() + OFF_VS_UNIF) as u64;
    let cs_unif = (arena_phys() + OFF_CS_UNIF) as u64;
    sf(&mut rec, 96, 1, 1); // FS 4-way threadable
    sf(&mut rec, 98, 1, 1); // FS propagate NaNs (v42)
    sf(&mut rec, 99, 29, fs >> 3); // FS code address
    sf(&mut rec, 128, 32, fs_unif); // FS uniforms address (PI-V3D-9: colour + TLB config stream)
    sf(&mut rec, 160, 1, 1); // VS 4-way threadable
    sf(&mut rec, 162, 1, 1); // VS propagate NaNs (v42)
    sf(&mut rec, 163, 29, vs >> 3); // VS code address
    sf(&mut rec, 192, 32, vs_unif); // VS uniforms address (PI-V3D-9: VPM read-offset stream)
    sf(&mut rec, 224, 1, 1); // CS 4-way threadable
    sf(&mut rec, 226, 1, 1); // CS propagate NaNs (v42)
    sf(&mut rec, 227, 29, cs >> 3); // CS code address
    sf(&mut rec, 256, 32, cs_unif); // CS uniforms address (PI-V3D-9: VPM read-offset stream)
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
    // TILE_BINNING_MODE_CFG (v42): 64×64 frame, 1 RT, no MSAA, no double-buffer, 32-bit max BPP.
    // PI-V3D-14: 128-byte INITIAL block + 64-byte overflow block — Mesa's only silicon-exercised
    // config (v3d_limits.h INITIAL=128/OVERFLOW=64; boot-P9 showed the binner never wrote the pool
    // under 64B/64B). Field bits: width@32(16,minus_one), height@48(16,minus_one),
    // num RT@8(4,minus_one), max bpp@12(2), block size@4(2), initial block size@2(2).
    w.pkt(
        Pkt::new(P_TILE_BINNING_MODE_CFG, 9)
            .f(2, 2, TILE_ALLOC_BLOCK_SIZE_128B) // tile allocation initial block size 128b
            .f(4, 2, TILE_ALLOC_BLOCK_SIZE_64B) // tile allocation (overflow) block size 64b
            .f(8, 4, 0) // Number of Render Targets (minus_one: 1 → 0)
            .f(12, 2, INTERNAL_BPP_32) // Maximum BPP of all render targets
            .f(32, 16, (TARGET_W - 1) as u64) // Width in pixels (minus_one)
            .f(48, 16, (TARGET_H - 1) as u64) // Height in pixels (minus_one)
            .done(),
    );
    // Flush any stale VCD, then START_TILE_BINNING (must precede geometry).
    w.pkt(Pkt::new(P_FLUSH_VCD_CACHE, 1).done());
    w.pkt(Pkt::new(P_START_TILE_BINNING, 1).done());

    // ── PI-V3D-17: clip/viewport/config state (V3D-16 verdict). Without these the hardware clipper
    // runs at power-on-reset zeros — zero viewport scale collapses every primitive to a point and the
    // binner writes an empty-but-legal bin (tile-alloc pool never touched). All opcodes/lengths/field
    // bits are VERBATIM from Mesa v3d_packet.xml gen 4.2; the transform values follow Mesa's own
    // v3dX(emit) (v3dx_emit.c / v3dvx_cmd_buffer.c) fixed-function viewport emit.
    //
    // CS-COORDS CONSISTENCY — PI-V3D-18 RESOLUTION (supersedes the earlier "shader OR fixed-function"
    // framing, which was a false dichotomy). Mesa's coordinate (bin) shader emits BOTH: the fixed-
    // function viewport state below AND two screen-space words the shader itself writes. Authoritative
    // layout — `v3d_nir_setup_vpm_layout_vs` / `v3d_nir_emit_ff_vpm_outputs`
    // (src/broadcom/compiler/v3d_nir_lower_io.c): for is_coord the VPM OUTPUT is SIX words —
    //     offset 0..3 : Xc, Yc, Zc, Wc     (raw clip-space position; state->pos[0..3])
    //     offset 4    : Xs = f2i32(floor( Xc · vp_scale_x · 1/Wc ))   ← screen X, .8 fixed-point, INT
    //     offset 5    : Ys = f2i32(floor( Yc · vp_scale_y · 1/Wc ))   ← screen Y
    // where vp_scale = viewport.scale · clipper_xy_granularity = 32 · 256 = 8192 (v3d_uniforms.c
    // QUNIFORM_VIEWPORT_X_SCALE; granularity 256.0f for ver 42, v3d_device_info.c). The Xs/Ys are
    // CENTRE-RELATIVE (no +centre in the shader); the +32,+32 centre is supplied by VIEWPORT_OFFSET
    // below — so the two mechanisms COMPOSE, they do not double-apply. The PTB bins from the SCREEN
    // words (out-offsets 4,5). PI-V3D-19 RESOLVES the residual V3D-17/18 empty-bin cause: CS_VS_WORDS now
    // emits all SIX words — the 4 clip words THEN Xs,Ys (fmul·8192 → ffloor → ftoiz → mov vpm, per axis),
    // Mesa-packed by scripts/pi-v3d19-qpu-gen.c. W=1 SIMPLIFICATION: TRI_VERTS all carry Wc=1.0, so
    // 1/Wc=1.0 and the transform is floor(coord·8192) with NO reciprocal (this holds ONLY for W=1
    // geometry). The fixed-function state below is CORRECT and stays; the record's VPM segment sizes are
    // also CORRECT (Mesa packs them in SECTORS = align(words,8)/8: 4 in → 1, 6 out → 1; vir.c
    // v3d_vs_set_prog_data — re-verified: 6 words still round to 1 sector, record unchanged). QEMU models
    // no V3D so the real verdict is the next metal boot's tile-alloc pool / tile-STATE going non-zero;
    // cs_vpm_output_witness() prints the expected Xs/Ys per vertex to check against.

    // CFG_BITS (code 96, v42): enable BOTH forward- and reverse-facing primitives (no cull); every
    // other bit 0 (no depth/stencil/blend). Fields: fwd-facing@0(1), rev-facing@1(1). Length 4
    // (opcode + 3 payload; max field bit 21 → 3 bytes).
    w.pkt(
        Pkt::new(P_CFG_BITS, 4)
            .f(0, 1, 1) // Enable Forward Facing Primitive
            .f(1, 1, 1) // Enable Reverse Facing Primitive
            .done(),
    );
    // CLIP_WINDOW (code 107): left=0, bottom=0, width=TARGET_W, height=TARGET_H. Fields:
    // left@0(16), bottom@16(16), width@32(16), height@48(16). Length 9 (opcode + 8 payload).
    w.pkt(
        Pkt::new(P_CLIP_WINDOW, 9)
            .f(0, 16, 0) // Clip Window Left Pixel Coordinate
            .f(16, 16, 0) // Clip Window Bottom Pixel Coordinate
            .f(32, 16, TARGET_W as u64) // Clip Window Width in pixels
            .f(48, 16, TARGET_H as u64) // Clip Window Height in pixels
            .done(),
    );
    // VIEWPORT_OFFSET (code 108): screen-space centre (32,32). Per v3dx_emit.c the fine coords hold
    // viewport.translate (the centre, in pixels) and coarse=0 for non-negative centres. Fine X/Y are
    // type u14.8 (value × 256): 32.0 px → 8192. Fields: fine_x@0(22,u14.8), coarse_x@22(10,int),
    // fine_y@32(22,u14.8), coarse_y@54(10,int). Length 9 (opcode + 8 payload; max field bit 63).
    const VP_FINE_CENTRE: u64 = (TARGET_W as u64 / 2) * 256; // 32 px × 256 = 8192 (u14.8)
    w.pkt(
        Pkt::new(P_VIEWPORT_OFFSET, 9)
            .f(0, 22, VP_FINE_CENTRE) // Fine X (u14.8): centre 32.0 px
            .f(22, 10, 0) // Coarse X (int): 0
            .f(32, 22, VP_FINE_CENTRE) // Fine Y (u14.8): centre 32.0 px
            .f(54, 10, 0) // Coarse Y (int): 0
            .done(),
    );
    // CLIPPER_XY_SCALING (code 110, v42): viewport half-extent in 1/256th px, as f32. Per
    // v3dx_emit.c the field is viewport.scale × 256.0f; half-width of a 64 px viewport = 32 px →
    // 32 × 256 = 8192.0f32. Fields: half-width@0(32,float), half-height@32(32,float). Length 9.
    let half_scale = (((TARGET_W as f32) / 2.0) * 256.0).to_bits() as u64; // 8192.0f32
    w.pkt(
        Pkt::new(P_CLIPPER_XY_SCALING, 9)
            .f(0, 32, half_scale) // Viewport Half-Width in 1/256th of pixel
            .f(32, 32, half_scale) // Viewport Half-Height in 1/256th of pixel
            .done(),
    );
    // CLIPPER_Z_SCALE_AND_OFFSET (code 111): map NDC z [-1,1] → depth [0,1]. Per v3dx_emit.c the
    // fields are viewport.scale[2] (=0.5) and viewport.translate[2] (=0.5). Fields:
    // z_scale@0(32,float), z_offset@32(32,float). Length 9.
    w.pkt(
        Pkt::new(P_CLIPPER_Z_SCALE_AND_OFFSET, 9)
            .f(0, 32, (0.5f32).to_bits() as u64) // Viewport Z Scale (Zc to Zs)
            .f(32, 32, (0.5f32).to_bits() as u64) // Viewport Z Offset (Zc to Zs)
            .done(),
    );

    // Draw state: VCM cache size (1 batch each for bin+render), the shader-state pointer, then the prim.
    w.pkt(
        Pkt::new(P_VCM_CACHE_SIZE, 2)
            .f(0, 4, 1) // 16-vertex batches for binning
            .f(4, 4, 1) // 16-vertex batches for rendering
            .done(),
    );
    // GL_SHADER_STATE: address is a 27-bit field @ start5 → the record's 32-byte-aligned address's top
    // 27 bits; number of attribute arrays in the low 5 bits.
    //
    // PI-V3D-10 boot-P6 root cause #1 (the out-of-arena bin fault): this packet was emitted with
    // length 4 — opcode + only THREE payload bytes — but the address field spans XML bits [5, 31], so
    // the payload is 4 bytes and the packet is 5 bytes total (v3d_packet.xml code 64). The CLE
    // therefore consumed the FOLLOWING packet's opcode byte — VERTEX_ARRAY_PRIMS, 36 = 0x24 — as the
    // shader-record address's top byte and fetched the record at 0x24000000 | shadrec. Boot-P6 proof:
    // VIO_ID 0x81 >> 5 = client 4 = CLE (v3d_irq.c v3d_41_axi_ids), and VIO_ADDR 0x04841800 scaled by
    // (va_width − 32) = 3 (DEBUG_INFO 0x550 → VA_WIDTH field 5 → va_width 35, per v3d_drv.c) gives
    // VA 0x2420C000 = 0x24 << 24 | 0x20C000 — exactly the shader record (arena+0x1C000) with the 0x24
    // opcode byte on top. The "POR-shaped garbage" was our own next opcode. Length corrected to 5.
    w.pkt(
        Pkt::new(P_GL_SHADER_STATE, 5)
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
            .f(0, 2, TILE_ALLOC_BLOCK_SIZE_128B) // PI-V3D-14: match bin config's initial block
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
/// Returns the M4 verdict (PI-V3D-12: `bringup` gates the battery on it — the battery stages layer on
/// the M4 scaffold, so running them over a failed triangle only buries the M4 witness in noise).
fn triangle_job(fb: Option<FbTarget>) -> bool {
    serial_println!(":: V3D: M4 triangle — binning on CT0, render on CT1 (implicit tile list) ::");

    // (0) Publish the shader programs, uniform streams, vertex data, default attributes. The shader
    // bodies are now REAL Mesa-packer-generated + round-trip-verified QPU words (PI-V3D-9), not NOPs:
    // coordinate/vertex passthrough (VPM in → VPM out) and a solid-colour fragment (rgba → TLB).
    let cs_len = write_shader_words(OFF_CS_CODE, &CS_VS_WORDS);
    let vs_len = write_shader_words(OFF_VS_CODE, &CS_VS_WORDS);
    let fs_len = write_shader_words(OFF_FS_CODE, &FS_WORDS);
    let fs_unif_len = write_fs_uniforms(OFF_FS_UNIF);
    let cs_unif_len = write_geo_uniforms(OFF_CS_UNIF);
    let vs_unif_len = write_geo_uniforms(OFF_VS_UNIF);
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
    cache::clean_range(arena_phys() + OFF_CS_CODE, cs_len);
    cache::clean_range(arena_phys() + OFF_VS_CODE, vs_len);
    cache::clean_range(arena_phys() + OFF_FS_CODE, fs_len);
    cache::clean_range(arena_phys() + OFF_FS_UNIF, fs_unif_len);
    cache::clean_range(arena_phys() + OFF_CS_UNIF, cs_unif_len);
    cache::clean_range(arena_phys() + OFF_VS_UNIF, vs_unif_len);
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

    // (4) Kick CT0 (the BIN queue). PI-V3D-9 boot-P5 fix: program the tile-ALLOCATION pool (CT0QMA/QMS)
    // AND the tile-STATE array (CT0QTS, ENABLE-gated) as the DISTINCT regions they are — the base
    // conflated them, handing the binner a 192-byte "pool" that overflowed into an unmapped page
    // (PT_INVALID). Order per Linux v3d_sched.c v3d_bin_job_run: QMA, QMS, QTS, then QBA (begin), then
    // QEA (GO). All addresses are arena-internal identity iovas, bounds-checked (memory-safety).
    let bin_ba = (arena_phys() + OFF_BIN_CL) as u32;
    let bin_ea = bin_ba + bin_len as u32;
    let tile_alloc = (arena_phys() + OFF_BIN_TILEALLOC) as u32; // the binner's growable pool (CT0QMA/QMS)
    let ts = (arena_phys() + OFF_TILESTATE) as u32; // the tile-state data array (CT0QTS)
    if !arena_contains(bin_ba as usize, bin_len)
        || !arena_contains(tile_alloc as usize, BIN_TILEALLOC_BYTES)
        || !arena_contains(ts as usize, TILE_STATE_BYTES)
    {
        serial_println!(":: V3D: M4 bin range escapes the arena — refusing kick (fail-closed) ::");
        return false;
    }
    // PI-V3D-15 (brief lead #1 attribution): clear any stale MMU fault BEFORE the bin kick so the
    // post-bin decode below is provably THIS bin's fault — the M4 bin clue's MMU_fault=0x100000 could
    // otherwise be a fault latched by program_mmu/M3 and never cleared (there was no pre-bin clear).
    // (brief lead #2): dump the exact bin CL byte stream the binner will parse, to read against Mesa's
    // emit order for a mis-sized packet shifting an opcode into an address field (PI-V3D-10 class).
    clear_mmu_fault_latch("v3d15 pre-bin (attribution)");
    dump_cl_bytes("M4 bin", OFF_BIN_CL, bin_len, 64);
    // PI-V3D-12: the Linux per-job pre-kick cache invalidate (v3d_bin_job_run does this first).
    invalidate_gpu_caches("L2T flush (M4 bin pre-kick)");
    let ct0_cs_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    let ct0_ca_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CA);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QMA, tile_alloc); // tile-allocation pool base
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QMS, BIN_TILEALLOC_BYTES as u32); // …and its size
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QTS, ts | V3D_CLE_CT0QTS_ENABLE); // tile-state array (enabled)
    dsb();
    // PI-V3D-13 witness: prove the bin-memory registers hold what we wrote BEFORE the GO.
    bin_mem_prekick_witness(
        "M4",
        tile_alloc,
        BIN_TILEALLOC_BYTES as u32,
        ts | V3D_CLE_CT0QTS_ENABLE,
    );
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
    // PI-V3D-13 witness: post-bin, did the binner's output actually land in the pool?
    bin_pool_witness("M4 post-bin");
    // PI-V3D-18 witness (V3D-16-mandated): the shader-state record bytes the CLE handed the PTB, plus
    // the CONTRACTED 6-word coordinate-shader VPM output vs. our 4-word passthrough — so every boot
    // shows the two screen-space words (out-offsets 4,5) the CS omits, the confirmed empty-bin cause.
    cs_vpm_output_witness("M4 post-bin");
    // PI-V3D-15 (brief lead #1): decode WHERE the bin faulted — the clue above reports the fault BITS
    // but never the address. With the latch cleared pre-kick, a fault here is THIS bin's, and its VA
    // tells whether the binner walked off the arena (our encoding bug) or idled legally in-bounds.
    bin_fault_witness("M4 bin");
    super::exceptions::serror_drain_request("v3d: M4 bin kick window");

    // PI-V3D-9 boot-P5 fix: clear any latched V3D-MMU fault BEFORE the render kick. Boot-P5 showed the
    // render CT1 refused to start (CTRUN never latched, CT1CA parked at M3's end) while the MMU carried
    // a latched PT_INVALID+WRITE_VIOLATION from the (then-broken) bin — a fault the abort policy holds
    // sticky, wedging subsequent submissions. The clear is the exact Linux v3d_irq.c idiom: read
    // V3D_MMU_CTL and write it back (the fault status bits are write-1-to-clear; writing the read-back
    // value clears them while preserving ENABLE/abort config). Harmless when no fault is latched (the
    // fault bits read 0, so the write-back is a no-op on them). With the bin fault fixed above this is
    // belt-and-suspenders; it also un-wedges the render if any unrelated fault slipped in.
    clear_mmu_fault_latch("post-bin");

    // (5) Kick CT1 (the RENDER queue) over the M4 RCL — same submit path as M3, different list. It
    // consumes the binner's per-tile lists via BRANCH_TO_IMPLICIT_TILE_LIST.
    let rcl_ba = (arena_phys() + OFF_M4_RCL) as u32;
    let rcl_ea = rcl_ba + rcl_len as u32;
    if !arena_contains(rcl_ba as usize, rcl_len) {
        serial_println!(":: V3D: M4 render range escapes the arena — refusing kick (fail-closed) ::");
        return false;
    }
    // PI-V3D-12 — THE boot-P7 fix. The render CLE consumes the BINNER's tile lists; without the Linux
    // per-job invalidate (v3d_render_job_run also runs it) the L2T still held the CPU's pre-bin
    // zero-fill of the tile-alloc pool, so the BRANCH_TO_IMPLICIT_TILE_LIST fetched 0x00 = Halt at the
    // pool base and the CLE stopped there (boot-P7: CT1CA done 0x00206000 = arena+OFF_BIN_TILEALLOC,
    // BELOW BA) without ever reaching the sub-list's STORE. Flush L2T + invalidate slices so the
    // render observes the bin's actual output.
    invalidate_gpu_caches("L2T flush (M4 render pre-kick)");
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
    // PI-V3D-9: decode the one CTnCS bit corroborated for V3D 4.x — CTRUN (bit5, "list running"). The
    // kicked snapshot having CTRUN set is the positive proof the render actually started (boot-P5 had
    // CTRUN clear here → the wedge the fault-latch clear above targets); other CTnCS bits are reported
    // raw, not guessed.
    let r_ctrun_kicked = (r_cs_kicked & V3D_CLE_CTNCS_CTRUN != 0) as u32;
    serial_println!(
        ":: V3D: M4 render clue — CT1CS pre={:#010x} kicked={:#010x} (CTRUN={}) done={:#010x} CT1CA done={:#010x} (BA={:#010x} EA={:#010x}) ran={} idled={} MMU_fault={:#x} ::",
        r_cs_pre, r_cs_kicked, r_ctrun_kicked, r_cs_done, r_ca_done, rcl_ba, rcl_ea, r_ran as u32, r_idled as u32, r_fault
    );
    // PI-V3D-12 CA-locus decode: CT1CA below BA is NOT a stale queued job — the CLE's CA follows
    // branches. Parked inside the bin tile-alloc pool = the BRANCH_TO_IMPLICIT_TILE_LIST destination,
    // i.e. the CLE halted INSIDE the (stale/empty) binned tile list before the sub-list's STORE.
    let ta_base = (arena_phys() + OFF_BIN_TILEALLOC) as u32;
    if r_ca_done >= ta_base && r_ca_done < ta_base + BIN_TILEALLOC_BYTES as u32 {
        serial_println!(
            ":: V3D: M4 render clue — CT1CA parked IN the bin tile-alloc pool (+{:#x}): the CLE halted inside the implicit (binned) tile list, before the STORE ::",
            r_ca_done - ta_base
        );
    }
    super::exceptions::serror_drain_request("v3d: M4 render kick window");

    if !bin_idled || !r_idled {
        serial_println!(":: V3D: M4 — a CLE did not idle within budget (anti-hang backstop) — no verify ::");
        return false;
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
    pass
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

// ════════════════════════════════════════════════════════════════════════════════════════════════
// PI-V3D-11 — the visible graphics battery (M5..M8), LAYERED on the M4 triangle scaffold.
// ════════════════════════════════════════════════════════════════════════════════════════════════
//
// Four short on-screen stages, each serial-witnessed (`:: V3D: M<stage> … ::`) + eyeball-verified at
// the attended sitting. Everything below is ADDITIVE: new arena regions above the M4 regions, new
// builder/kick functions that mirror (not modify) the M4 idiom, and one call in `bringup`. The M3
// clear and M4 triangle above stay byte-identical as the head-of-battery regressions.
// ATTENDED-METAL-UNVERIFIED throughout — QEMU raspi4b returns at BLOCK-DOWN long before this runs.
//
// ── QPU-word provenance (the standing thrice-convicted rule — no fabricated bit patterns) ────────
// Every NEW 64-bit word below is derived from the PI-V3D-9 Mesa-packer-verified vectors already in
// this file by SINGLE-FIELD surgery, where the touched field's encoding is itself corroborated by
// multiple in-file verified words (the same "CT1 = CT0 + 4" class the CT0 registers used):
//   * SIG field [57:53]: corroborated by nop(sig=0), thrsw(sig=1 → bit53, in-file 0x3c20…) and
//     ldunifrf(sig=12 → bits56+55, in-file 0x3d80…). ldvary is sig=8 (Mesa qpu_pack.c v41 sig map,
//     the same table that yields 1/12 for the corroborated entries) → bit56 alone.
//   * SIG dest addr [51:46] (rf#): corroborated by the in-file ldunifrf.rf0..rf3 (+0x0000/bit46/
//     bit47/bits46+47) and ldunifrf.rf5 words.
//   * WADDR_A [37:32]: corroborated by the in-file ldvpmv_in rf0..rf3 sequence (+0..3 at bit 32).
//   * RADDR_A [11:6]: corroborated by the in-file `mov vpm, rf0..rf3` sequence (+0x00/0x40/0x80/0xc0).
// The gradient-FS SEMANTICS (raw ldvary A-coefficients written to the TLB without the fmul/fadd(W,C)
// interpolation evaluation) are the honest metal-refinement seam of this arc, exactly like the
// PI-V3D-9 viewport/VPM quantities — flagged at the M5 verdict.

// ─── Battery arena regions: 0x21000..0x34000, all 4 KiB-aligned starts, all ABOVE the M4 regions
// (top prior used byte ≈ 0x200C0) and inside the 256 KiB arena → inside the identity MMU map. ───
const OFF_M5_TARGET: usize = 0x21000; // [16 KiB) M5 gradient render target
const OFF_M5_VTX: usize = 0x25000; // 3 verts × 32 B (vec4 pos + vec4 colour, interleaved)
const OFF_M5_FS_CODE: usize = 0x25800; // gradient fragment shader (ldvary path)
const OFF_M5_VS_CODE: usize = 0x26000; // gradient vertex shader (8-word VPM passthrough)
const OFF_M5_FS_UNIF: usize = 0x26800; // gradient FS uniforms (alpha + TLB configs)
const OFF_M5_VS_UNIF: usize = 0x26880; // gradient VS uniforms (8 VPM read-offsets)
const OFF_M5_SHADREC: usize = 0x26900; // M5 shader record + 2 attribute records (32-B aligned)
const OFF_M5_BIN_CL: usize = 0x27000;
const OFF_M5_RCL: usize = 0x28000;
const OFF_M5_SUBLIST: usize = 0x29000;
const OFF_BAT_TARGET: usize = 0x2A000; // [16 KiB) shared M6/M7 render target
const OFF_BAT_VTX: usize = 0x2E000; // animated / multi-primitive vertex data
const OFF_BAT_BIN_CL: usize = 0x2F000;
const OFF_BAT_RCL: usize = 0x30000;
const OFF_BAT_SUBLIST: usize = 0x31000;
const OFF_BAT_SHADREC: usize = 0x32000; // M6 record @+0; M7 records @+128×k (k=0..3; 52 B each)
const OFF_M7_UNIF: usize = 0x33000; // 4 FS uniform streams, 64-B stride (one per M7 colour draw)
const _: () = assert!(OFF_M7_UNIF + 4 * 64 <= ARENA_BYTES);

// ─── Battery QPU shader bodies (field-surgery derivations — provenance in the banner above). ───

/// M5 gradient FRAGMENT shader: pop three varyings (r, g, b A-coefficients) via ldvary into rf0..rf2,
/// alpha from the uniform FIFO into rf3, then the same passthrough-Z + double-VFPACK TLB write as the
/// verified FS_WORDS. Uniform FIFO order: alpha, Z-config, colour-config. Metal-refinement seam: the
/// varying interpolation math (fmul/fadd with W and the C coefficient) is NOT evaluated — the raw
/// per-fragment ldvary results land in the TLB, which is sufficient for the M5 witness (three
/// pairwise-distinct non-clear interior samples) but not yet colour-exact.
const GRAD_FS_WORDS: [u64; 10] = [
    0x3d00_3186_bb80_0000, // nop ; ldvary.rf0   (varying 0 → r)
    0x3d00_7186_bb80_0000, // nop ; ldvary.rf1   (varying 1 → g)
    0x3d00_b186_bb80_0000, // nop ; ldvary.rf2   (varying 2 → b)
    0x3d80_f186_bb80_0000, // nop ; ldunifrf.rf3 (rf3 <- alpha)   [verbatim in-file word]
    0x3c00_3206_bbe0_0000, // mov tlbu, r0       (passthrough-Z; pops Z TLB-config)
    0x3c00_3188_3583_e001, // vfpack tlbu, rf0, rf1 (pops colour TLB-config)
    0x3c00_3187_3583_e083, // vfpack tlb, rf2, rf3
    0x3c20_3186_bb80_0000, // nop ; thrsw
    0x3c00_3186_bb80_0000, // nop
    0x3c00_3186_bb80_0000, // nop
];

/// M5 gradient VERTEX shader: the CS_VS_WORDS passthrough widened to EIGHT VPM words (vec4 position +
/// vec4 colour varying source). Destinations rf0..rf3 + rf6..rf9 (rf4/rf5 skipped: rf5 is the live
/// read-offset register the ldunifrf reload targets — a dest collision would clobber it). Each word is
/// its CS_VS_WORDS counterpart with only WADDR_A (ldvpmv) or RADDR_A (mov vpm) advanced.
const GRAD_VS_WORDS: [u64; 21] = [
    0x3d81_6180_bc80_6140, // ldvpmv_in rf0, rf5 ; ldunifrf.rf5   (pos.x)  [verbatim]
    0x3d81_6181_bc80_6140, // ldvpmv_in rf1, rf5 ; ldunifrf.rf5   (pos.y)  [verbatim]
    0x3d81_6182_bc80_6140, // ldvpmv_in rf2, rf5 ; ldunifrf.rf5   (pos.z)  [verbatim]
    0x3d81_6183_bc80_6140, // ldvpmv_in rf3, rf5 ; ldunifrf.rf5   (pos.w)  [verbatim]
    0x3d81_6186_bc80_6140, // ldvpmv_in rf6, rf5 ; ldunifrf.rf5   (col.r)  [WADDR_A 3→6]
    0x3d81_6187_bc80_6140, // ldvpmv_in rf7, rf5 ; ldunifrf.rf5   (col.g)  [WADDR_A 3→7]
    0x3d81_6188_bc80_6140, // ldvpmv_in rf8, rf5 ; ldunifrf.rf5   (col.b)  [WADDR_A 3→8]
    0x3d81_6189_bc80_6140, // ldvpmv_in rf9, rf5 ; ldunifrf.rf5   (col.a)  [WADDR_A 3→9]
    0x3c00_3186_bb81_e140, // vpmsetup -, rf5     [verbatim]
    0x3c00_3386_bbf8_0000, // mov vpm, rf0        (pos.x)  [verbatim]
    0x3c00_3386_bbf8_0040, // mov vpm, rf1        (pos.y)  [verbatim]
    0x3c00_3386_bbf8_0080, // mov vpm, rf2        (pos.z)  [verbatim]
    0x3c00_3386_bbf8_00c0, // mov vpm, rf3        (pos.w)  [verbatim]
    0x3c00_3386_bbf8_0180, // mov vpm, rf6        (col.r)  [RADDR_A 3→6]
    0x3c00_3386_bbf8_01c0, // mov vpm, rf7        (col.g)  [RADDR_A 3→7]
    0x3c00_3386_bbf8_0200, // mov vpm, rf8        (col.b)  [RADDR_A 3→8]
    0x3c00_3386_bbf8_0240, // mov vpm, rf9        (col.a)  [RADDR_A 3→9]
    0x3c00_3186_bb81_6000, // vpmwt               [verbatim]
    0x3c20_3186_bb80_0000, // nop ; thrsw
    0x3c00_3186_bb80_0000, // nop
    0x3c00_3186_bb80_0000, // nop
];

/// The M5 per-vertex colours (unorm8 RGBA words) — one primary per corner, so interpolation (or even
/// raw per-fragment varying data) yields three PAIRWISE-DISTINCT interior samples near the corners.
const M5_VERT_COLOURS: [u32; 3] = [0x0000_00FF, 0x0000_FF00, 0x00FF_0000]; // red, green, blue

/// M6 animation cadence: 24 rotation steps × 6 revolutions ≈ 5 s at ~33 ms/frame.
const M6_FRAMES: usize = 144;
const M6_FRAME_PACE_MS: u64 = 30;

/// 24-step unit-circle table (cos, sin at k×15°), f32 — no libm in the kernel; precomputed.
const ROT24: [(f32, f32); 24] = [
    (1.0, 0.0),
    (0.965926, 0.258819),
    (0.866025, 0.5),
    (0.707107, 0.707107),
    (0.5, 0.866025),
    (0.258819, 0.965926),
    (0.0, 1.0),
    (-0.258819, 0.965926),
    (-0.5, 0.866025),
    (-0.707107, 0.707107),
    (-0.866025, 0.5),
    (-0.965926, 0.258819),
    (-1.0, 0.0),
    (-0.965926, -0.258819),
    (-0.866025, -0.5),
    (-0.707107, -0.707107),
    (-0.5, -0.866025),
    (-0.258819, -0.965926),
    (0.0, -1.0),
    (0.258819, -0.965926),
    (0.5, -0.866025),
    (0.707107, -0.707107),
    (0.866025, -0.5),
    (0.965926, -0.258819),
];

/// M7 draw colours (one per 3-wedge group of the 12-triangle pinwheel): red, green, blue, amber.
const M7_COLOURS: [u32; 4] = [0x0000_00FF, 0x0000_FF00, 0x00FF_0000, TRI_RGBA];

/// The battery sentinel — distinct from CLEAR_RGBA, TRI_RGBA and every M5/M7 draw colour, so a
/// sample equal to it proves "GPU never wrote this pixel".
const BAT_SENTINEL: u32 = 0x5555_5555;

/// One bin→render job outcome, shared by every battery stage (the M4 kick idiom, parameterised).
struct JobResult {
    bin_ran: bool,
    bin_idled: bool,
    r_ran: bool,
    r_idled: bool,
    /// OR of the MMU fault-status bits observed after the bin and after the render.
    fault: u32,
}
impl JobResult {
    fn clean(&self) -> bool {
        self.bin_ran && self.bin_idled && self.r_ran && self.r_idled && self.fault == 0
    }
}

/// Quiet variant of `clear_mmu_fault_latch`: same Linux v3d_irq.c read-echo W1C idiom, but returns the
/// latched fault bits instead of printing — the M6 frame loop calls this per frame and 144 lines of
/// latch chatter would bury the serial witness (quiet-boot law). A non-zero return is reported once in
/// the stage verdict.
fn clear_mmu_fault_latch_quiet() -> u32 {
    let ctl = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let fault = ctl & (V3D_MMU_CTL_PT_INVALID | V3D_MMU_CTL_WRITE_VIOLATION | V3D_MMU_CTL_CAP_EXCEEDED);
    if fault != 0 {
        mmio_write(V3D_HUB_BASE, V3D_MMU_CTL, ctl); // W1C echo
        dsb();
    }
    fault
}

/// PI-V3D-13 pre-kick witness: read back the three bin-memory registers just written (CT0QMA =
/// tile-allocation pool base, CT0QMS = its size, CT0QTS = tile-state array base | ENABLE). The
/// PI-V3D-13 fact-check confirmed the programming model against Linux v3d_regs.h/v3d_sched.c
/// verbatim (offsets 0x170/0x174/0x15c, ENABLE=BIT(1), order QMA→QMS→QTS→QBA→QEA-GO), so a readback
/// that does NOT echo what we wrote is itself the boot-P8 clue: either the slots are not where the
/// silicon holds them or the writes are not landing.
fn bin_mem_prekick_witness(tag: &str, qma_w: u32, qms_w: u32, qts_w: u32) {
    let qma = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0QMA);
    let qms = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0QMS);
    let qts = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0QTS);
    serial_println!(
        ":: V3D: {} bin-mem regs — CT0QMA={:#010x} (wrote {:#010x}) CT0QMS={:#010x} (wrote {:#010x}) CT0QTS={:#010x} (wrote {:#010x}) echo={} ::",
        tag, qma, qma_w, qms, qms_w, qts, qts_w,
        (qma == qma_w && qms == qms_w && qts == qts_w) as u32
    );
}

/// PI-V3D-13 post-bin witness: CPU-read the tile-alloc pool head and give the one-line verdict the
/// brief asks for — did the BINNER actually write its output into the pool? The CPU zero-filled and
/// cleaned the pool pre-kick, so its D-cache holds zero lines over the head; clean+invalidate the
/// head line first (clean is a no-op — the lines were cleaned at publish — and the invalidate makes
/// this read observe the binner's DRAM write). Nonzero head bytes = the binner wrote a tile list.
fn bin_pool_witness(tag: &str) -> bool {
    cache::clean_invalidate_range(arena_phys() + OFF_BIN_TILEALLOC, 64);
    let arena = &raw const V3D_ARENA;
    let mut head = [0u8; 8];
    unsafe {
        for (i, h) in head.iter_mut().enumerate() {
            *h = (*arena).bytes[OFF_BIN_TILEALLOC + i];
        }
    }
    let wrote = head.iter().any(|&b| b != 0);
    serial_println!(
        ":: V3D: {} tile-alloc pool[0..8] = {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} — {} ::",
        tag, head[0], head[1], head[2], head[3], head[4], head[5], head[6], head[7],
        if wrote { "nonzero: the binner WROTE the pool" } else { "all zero: the binner never wrote the pool" }
    );
    // PI-V3D-17 (V3D-16 ask): dump the tile-STATE array head (CT0QTS) alongside the pool. The PTB
    // writes per-tile state (TSDA) here as it bins; nonzero here corroborates the pool witness.
    cache::clean_invalidate_range(arena_phys() + OFF_TILESTATE, 8);
    let mut ts = [0u8; 8];
    unsafe {
        for (i, t) in ts.iter_mut().enumerate() {
            *t = (*arena).bytes[OFF_TILESTATE + i];
        }
    }
    serial_println!(
        ":: V3D: {} tile-STATE[0..8] = {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} — {} ::",
        tag, ts[0], ts[1], ts[2], ts[3], ts[4], ts[5], ts[6], ts[7],
        if ts.iter().any(|&b| b != 0) { "nonzero: the PTB wrote tile-state" } else { "all zero: no tile-state written" }
    );
    wrote
}

/// PI-V3D-18 witness (V3D-16-mandated post-bin CS/VPM audit). Two records the next metal boot reads
/// to confirm what the hardware actually consumed for the coordinate (bin) shader:
///
///  (1) the 52 shader-state bytes at OFF_SHADREC — the 36-byte GL Shader State Record + the 16-byte
///      GL Shader State Attribute Record — the exact bytes the CLE's GL_SHADER_STATE fetch handed the
///      PTB. (The coordinate shader's VPM OUTPUT is on-chip and NOT CPU/DRAM-readable — there is no
///      V3D_VPM CPU window for per-QPU shader output on 4.x; Mesa reads VPM back only via LDVPM inside a
///      shader, never from the CPU — so the record bytes + the tile-alloc pool/tile-STATE are the only
///      readable witnesses of what the hardware did, per the V3D-16 fallback ask. PI-V3D-20: a TMU-store
///      readback debug variant was considered and SKIPPED — it is not trivial and would build a debug
///      subsystem the brief forbids; the pool/tile-STATE going non-zero remains the decisive verdict.)
///
///  (2) the CONTRACTED coordinate-shader VPM output vs. what CS_VS_WORDS actually emits, per vertex.
///      Mesa `v3d_nir_setup_vpm_layout_vs` (src/broadcom/compiler/v3d_nir_lower_io.c): for is_coord
///      the output layout is SIX words — pos[0..3] = clip Xc,Yc,Zc,Wc at offsets 0..3, THEN the two
///      screen-space words the PTB bins from at offsets 4,5: Xs = f2i32(floor(Xc·vp_scale_x·(1/Wc))),
///      Ys = f2i32(floor(Yc·vp_scale_y·(1/Wc))) (floor path is the ver==42 branch in
///      `v3d_nir_emit_ff_vpm_outputs`; f2i gives INTEGER .8 fixed-point, CENTRE-RELATIVE — the centre
///      is added by the fixed-function VIEWPORT_OFFSET). vp_scale = viewport.scale·clipper_xy_granularity
///      = 32 · 256 = 8192 (v3d_uniforms.c QUNIFORM_VIEWPORT_X_SCALE; granularity 256.0f for ver 42,
///      v3d_device_info.c). PI-V3D-20 stores all six output words via STVPMV at explicit VPM offsets
///      0..5 (screen Xs/Ys = fmul·8192 → ffloor → ftoiz, W=1 so no 1/Wc), Mesa-packed by
///      scripts/pi-v3d20-qpu-gen.c — correcting the V3D-9/19 mov-vpm/vpmsetup streamed path, which is not
///      the v42 output mechanism and wrote nowhere the PTB reads. This line prints the expected Xs/Ys per
///      vertex so the next metal boot can check the PTB's binned coords
///      against them (the CS VPM output is on-chip; the tile-alloc pool / tile-STATE going non-zero is
///      the real verdict).
fn cs_vpm_output_witness(tag: &str) {
    // (1) shader-state record + attribute record bytes (36 + 16 = 52).
    cache::clean_invalidate_range(arena_phys() + OFF_SHADREC, 52);
    dump_shadrec_bytes(tag, OFF_SHADREC, 52);
    // (2) contracted 6-word CS output vs. our 4-word passthrough, per vertex. Center-relative screen
    // coords (Mesa's shader math; VIEWPORT_OFFSET adds the +32,+32 centre in fixed function).
    let vp_scale: f64 = ((TARGET_W as f64) / 2.0) * 256.0; // 8192.0
    for (i, v) in TRI_VERTS.iter().enumerate() {
        let (xc, yc, zc, wc) = (v[0] as f64, v[1] as f64, v[2] as f64, v[3] as f64);
        let rcp_wc = if wc != 0.0 { 1.0 / wc } else { 0.0 };
        let xs = floor_i32(xc * vp_scale * rcp_wc);
        let ys = floor_i32(yc * vp_scale * rcp_wc);
        serial_println!(
            ":: V3D: [v3d20] {} CS-out v{} — CONTRACT[6] Xc={} Yc={} Zc={} Wc={} | Xs={} Ys={} (centre-rel .8fp) — CS_VS_WORDS now STORES all 6 via STVPMV @explicit out-offsets 0..5 (was mov-vpm/vpmsetup: wrong mechanism for v42, wrote nowhere); PTB should bin these ::",
            tag, i,
            (xc * 1000.0) as i32, (yc * 1000.0) as i32, (zc * 1000.0) as i32, (wc * 1000.0) as i32,
            xs, ys
        );
    }
}

/// floor(x) → i32 without libm (kernel no_std). `as i64` truncates toward zero; adjust down for
/// negatives with a fractional part to get a true floor.
#[inline]
fn floor_i32(x: f64) -> i32 {
    let t = x as i64;
    let f = if x < 0.0 && (t as f64) != x { t - 1 } else { t };
    f as i32
}

/// Hex-dump `n` arena bytes at `off` under the [v3d18] tag (the shader-state record witness; the CL
/// dumper's tag is fixed to [v3d15], so this dedicated copy keeps the arc tag correct).
fn dump_shadrec_bytes(tag: &str, off: usize, n: usize) {
    let arena = &raw const V3D_ARENA;
    serial_println!(
        ":: V3D: [v3d18] {} shader-state record — {} bytes @ arena+{:#x} (36 B record + 16 B attr) ::",
        tag, n, off
    );
    let mut i = 0;
    while i < n {
        let mut line = [0u8; 16];
        let mut c = 0;
        while c < 16 && i + c < n {
            line[c] = unsafe { (*arena).bytes[off + i + c] };
            c += 1;
        }
        serial_println!(
            "::   [v3d18]   +{:#05x}: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} ::",
            i, line[0], line[1], line[2], line[3], line[4], line[5], line[6], line[7],
            line[8], line[9], line[10], line[11], line[12], line[13], line[14], line[15]
        );
        i += 16;
    }
}

/// PI-V3D-15 fault witness (brief lead #1). The M4 bin clue reported the MMU fault BITS
/// (MMU_fault=0x100000 = PT_INVALID) but never WHERE. Read-only decode (does NOT clear): report the
/// violating AXI client (VIO_ID), the true faulting VA (VIO_ADDR un-shifted via DEBUG_INFO va_width),
/// the ILLEGAL_ADDR trap slot, and — the discriminator — whether that VA lies INSIDE the identity-
/// mapped arena. Inside-arena = not a confinement escape (a CL/shader address or a legally-idle bin);
/// outside-arena = the binner walked off the mapped region, i.e. a mis-encoded CL address field (the
/// PI-V3D-10 boot-P6 class). Reads-only; QEMU-safe (CTL reads 0/absent → "no fault latched").
fn bin_fault_witness(tag: &str) {
    let ctl = mmio_read(V3D_HUB_BASE, V3D_MMU_CTL);
    let fault = ctl & (V3D_MMU_CTL_PT_INVALID | V3D_MMU_CTL_WRITE_VIOLATION | V3D_MMU_CTL_CAP_EXCEEDED);
    let vio_addr = mmio_read(V3D_HUB_BASE, V3D_MMU_VIO_ADDR);
    let vio_id = mmio_read(V3D_HUB_BASE, V3D_MMU_VIO_ID);
    let illegal = mmio_read(V3D_HUB_BASE, V3D_MMU_ILLEGAL_ADDR);
    let dbg = mmio_read(V3D_HUB_BASE, V3D_MMU_DEBUG_INFO);
    let (client, va) = vio_decode(vio_id, vio_addr);
    let base = arena_phys() as u64;
    let top = base + ARENA_BYTES as u64;
    let locus = if fault == 0 {
        "no fault latched"
    } else if va >= base && va < top {
        "faulting VA INSIDE arena — confinement-legal (CL/shader address or legally-idle bin), NOT an out-of-arena walk-off"
    } else {
        "faulting VA OUTSIDE arena — the binner walked off the mapped region (mis-encoded CL address field: PI-V3D-10 class)"
    };
    serial_println!(
        ":: V3D: [v3d15] {} MMU fault decode — CTL={:#010x} (PT_INVALID={} WRITE_VIOLATION={} CAP_EXCEEDED={}) client={} VIO_ADDR={:#010x} VIO_ID={:#010x} ILLEGAL_ADDR={:#010x} DEBUG={:#010x} -> VA={:#012x} arena=[{:#012x},{:#012x}) — {} ::",
        tag, ctl,
        (fault & V3D_MMU_CTL_PT_INVALID != 0) as u32,
        (fault & V3D_MMU_CTL_WRITE_VIOLATION != 0) as u32,
        (fault & V3D_MMU_CTL_CAP_EXCEEDED != 0) as u32,
        client, vio_addr, vio_id, illegal, dbg, va, base, top, locus
    );
}

/// PI-V3D-15 CL byte-dump witness (brief lead #2). Hex-dump the emitted control-list bytes (bounded to
/// `cap`) so the exact packet stream the binner parses is on the wire — a mis-sized packet that shifts
/// a following opcode byte into an address field (the PI-V3D-10 boot-P6 GL_SHADER_STATE fault) is
/// visible here as the wrong bytes at the wrong offset when read against Mesa's emit order. Reads the
/// arena bytes the CPU just wrote (pre-kick); 16 bytes per line, tail bytes past the count are padding.
fn dump_cl_bytes(tag: &str, off: usize, len: usize, cap: usize) {
    let arena = &raw const V3D_ARENA;
    let n = len.min(cap);
    serial_println!(
        ":: V3D: [v3d15] {} CL byte stream — {} of {} bytes @ arena+{:#x} ::",
        tag, n, len, off
    );
    let mut i = 0;
    while i < n {
        let mut line = [0u8; 16];
        let mut c = 0;
        while c < 16 && i + c < n {
            line[c] = unsafe { (*arena).bytes[off + i + c] };
            c += 1;
        }
        serial_println!(
            "::   [v3d15]   +{:#05x}: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} ::",
            i, line[0], line[1], line[2], line[3], line[4], line[5], line[6], line[7],
            line[8], line[9], line[10], line[11], line[12], line[13], line[14], line[15]
        );
        i += 16;
    }
}

/// Kick one bin (CT0) + render (CT1) job pair over already-built, already-published control lists.
/// Mirrors the M4 kick sequence exactly (QMA/QMS/QTS → QBA → QEA-GO on CT0; QBA → QEA-GO on CT1;
/// finite backstops; fault-latch clear between the two) without touching the M4 code — so a V3D-10
/// change to the M4 kick path composes by mirroring the same fix here at rebase. The tile-alloc pool
/// and tile-state array are re-zeroed + re-published per call (the binner scribbles both).
fn kick_bin_render(bin_off: usize, bin_len: usize, rcl_off: usize, rcl_len: usize) -> JobResult {
    let mut res = JobResult { bin_ran: false, bin_idled: false, r_ran: false, r_idled: false, fault: 0 };

    let bin_ba = (arena_phys() + bin_off) as u32;
    let bin_ea = bin_ba + bin_len as u32;
    let rcl_ba = (arena_phys() + rcl_off) as u32;
    let rcl_ea = rcl_ba + rcl_len as u32;
    let tile_alloc = (arena_phys() + OFF_BIN_TILEALLOC) as u32;
    let ts = (arena_phys() + OFF_TILESTATE) as u32;
    if !arena_contains(bin_ba as usize, bin_len) || !arena_contains(rcl_ba as usize, rcl_len) {
        serial_println!(":: V3D: battery job range escapes the arena — refusing kick (fail-closed) ::");
        return res;
    }

    // Fresh binner scratch (same regions the M4 job used — free for reuse once M4 has completed).
    fill_region(OFF_TILESTATE, TILE_STATE_BYTES, 0);
    fill_region(OFF_BIN_TILEALLOC, BIN_TILEALLOC_BYTES, 0);
    cache::clean_range(arena_phys() + OFF_TILESTATE, TILE_STATE_BYTES);
    cache::clean_range(arena_phys() + OFF_BIN_TILEALLOC, BIN_TILEALLOC_BYTES);

    // CT0 (bin): QMA/QMS/QTS → QBA → QEA (GO), per Linux v3d_sched.c v3d_bin_job_run — which starts
    // with the per-job cache invalidate (PI-V3D-12 mirror of the M4 kick fix).
    invalidate_gpu_caches("L2T flush (battery bin pre-kick)");
    // PI-V3D-15 mirror (V3D-11 law): the M4 kick clears any stale MMU fault BEFORE the bin so a post-
    // bin fault is attributable. Mirror it here QUIETLY — the battery runs per-frame and a verbose
    // decode/dump per frame would bury the serial witness (quiet-boot law); the verbose [v3d15] decode
    // stays on the one-shot M4 discriminator path. Accumulate any pre-existing fault into res.fault.
    res.fault |= clear_mmu_fault_latch_quiet();
    let cs_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QMA, tile_alloc);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QMS, BIN_TILEALLOC_BYTES as u32);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QTS, ts | V3D_CLE_CT0QTS_ENABLE);
    dsb();
    // PI-V3D-13 witness mirror of the M4 kick path.
    bin_mem_prekick_witness(
        "battery",
        tile_alloc,
        BIN_TILEALLOC_BYTES as u32,
        ts | V3D_CLE_CT0QTS_ENABLE,
    );
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QBA, bin_ba);
    dsb();
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT0QEA, bin_ea); // GO
    dsb();
    let cs_kicked = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    res.bin_idled = wait_bit_clear(V3D_CORE0_BASE, V3D_CLE_CT0CS, V3D_CLE_CTNCS_CTRUN, "CT0 battery bin");
    let cs_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CS);
    let ca_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT0CA);
    res.bin_ran = ct0_ran(cs_pre, cs_kicked, cs_done, ca_done, bin_ba, bin_ea);
    // PI-V3D-13 witness mirror of the M4 kick path.
    bin_pool_witness("battery post-bin");
    // Fault-latch hygiene between bin and render (the boot-P5 sticky-fault wedge), quiet per-frame.
    res.fault |= clear_mmu_fault_latch_quiet();

    // CT1 (render): QBA → QEA (GO). PI-V3D-12: the pre-kick invalidate here is what publishes the
    // bin's tile lists to the render CLE's branch fetch (the boot-P7 zero-stores root cause).
    invalidate_gpu_caches("L2T flush (battery render pre-kick)");
    let r_cs_pre = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CS);
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT1QBA, rcl_ba);
    dsb();
    mmio_write(V3D_CORE0_BASE, V3D_CLE_CT1QEA, rcl_ea); // GO
    dsb();
    let r_cs_kicked = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CS);
    res.r_idled = wait_bit_clear(V3D_CORE0_BASE, V3D_CLE_CT1CS, V3D_CLE_CT1CS_CTRUN, "CT1 battery render");
    let r_cs_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CS);
    let r_ca_done = mmio_read(V3D_CORE0_BASE, V3D_CLE_CT1CA);
    res.r_ran = ct0_ran(r_cs_pre, r_cs_kicked, r_cs_done, r_ca_done, rcl_ba, rcl_ea);
    res.fault |= clear_mmu_fault_latch_quiet();
    res
}

/// A generalised GL Shader State Record + attribute records, mirroring `build_shader_record` with the
/// addresses/counts parameterised. `attrs`: (data address, stride, values read by CS, values read by
/// VS) per attribute record. Writes record + attribute records at `rec_off`; returns attr count.
fn build_shader_record_at(
    rec_off: usize,
    cs_off: usize,
    vs_off: usize,
    fs_off: usize,
    fs_unif_off: usize,
    vs_unif_off: usize,
    cs_unif_off: usize,
    num_varyings: u64,
    attrs: &[(usize, u32, u64, u64)],
) -> u32 {
    let cs = (arena_phys() + cs_off) as u64;
    let vs = (arena_phys() + vs_off) as u64;
    let fs = (arena_phys() + fs_off) as u64;
    let defaults = (arena_phys() + OFF_DEFAULT_ATTRS) as u64;

    let mut rec = [0u8; 36];
    sf(&mut rec, 1, 1, 1); // Enable clipping
    sf(&mut rec, 24, 8, num_varyings); // Number of varyings in Fragment Shader
    sf(&mut rec, 32, 4, 1); // Coord Shader output VPM segment size
    sf(&mut rec, 40, 4, 1); // Coord Shader input VPM segment size
    sf(&mut rec, 48, 4, 1); // Vertex Shader output VPM segment size
    sf(&mut rec, 56, 4, 1); // Vertex Shader input VPM segment size
    sf(&mut rec, 64, 32, defaults);
    sf(&mut rec, 96, 1, 1); // FS 4-way threadable
    sf(&mut rec, 98, 1, 1); // FS propagate NaNs
    sf(&mut rec, 99, 29, fs >> 3);
    sf(&mut rec, 128, 32, (arena_phys() + fs_unif_off) as u64);
    sf(&mut rec, 160, 1, 1); // VS 4-way threadable
    sf(&mut rec, 162, 1, 1);
    sf(&mut rec, 163, 29, vs >> 3);
    sf(&mut rec, 192, 32, (arena_phys() + vs_unif_off) as u64);
    sf(&mut rec, 224, 1, 1); // CS 4-way threadable
    sf(&mut rec, 226, 1, 1);
    sf(&mut rec, 227, 29, cs >> 3);
    sf(&mut rec, 256, 32, (arena_phys() + cs_unif_off) as u64);
    arena_write_bytes(rec_off, &rec);

    for (i, &(addr_off, stride, cs_reads, vs_reads)) in attrs.iter().enumerate() {
        let mut attr = [0u8; 16];
        sf(&mut attr, 0, 32, (arena_phys() + addr_off) as u64);
        sf(&mut attr, 32, 2, 3); // Vec size (4 components)
        sf(&mut attr, 34, 3, 2); // Type = Attribute float
        sf(&mut attr, 40, 4, cs_reads);
        sf(&mut attr, 44, 4, vs_reads);
        sf(&mut attr, 64, 32, stride as u64);
        sf(&mut attr, 96, 32, 0xFFFF); // Maximum Index
        arena_write_bytes(rec_off + 36 + i * 16, &attr);
    }
    cache::clean_range(arena_phys() + rec_off, 36 + attrs.len() * 16);
    attrs.len() as u32
}

/// A generalised binning control list, mirroring `build_bin_cl` with the list offset and DRAWS
/// parameterised: `draws` = (shader-record offset, attr count, first vertex, vertex count) per draw —
/// M5/M6 issue one draw, M7 issues four (one per colour group). Returns the list byte length.
fn build_bin_cl_at(cl_off: usize, draws: &[(usize, u32, u32, u32)]) -> usize {
    let mut w = RclWriter::new(cl_off);
    w.pkt(Pkt::new(P_NUMBER_OF_LAYERS, 2).f(0, 8, 0).done());
    w.pkt(
        Pkt::new(P_TILE_BINNING_MODE_CFG, 9)
            .f(2, 2, TILE_ALLOC_BLOCK_SIZE_128B) // PI-V3D-14: 128B initial (Mesa-exercised config)
            .f(4, 2, TILE_ALLOC_BLOCK_SIZE_64B) // 64B overflow (Mesa OVERFLOW_BLOCK_SIZE)
            .f(8, 4, 0)
            .f(12, 2, INTERNAL_BPP_32)
            .f(32, 16, (TARGET_W - 1) as u64)
            .f(48, 16, (TARGET_H - 1) as u64)
            .done(),
    );
    w.pkt(Pkt::new(P_FLUSH_VCD_CACHE, 1).done());
    w.pkt(Pkt::new(P_START_TILE_BINNING, 1).done());
    w.pkt(Pkt::new(P_VCM_CACHE_SIZE, 2).f(0, 4, 1).f(4, 4, 1).done());
    for &(rec_off, num_attrs, first, count) in draws {
        let shadrec = (arena_phys() + rec_off) as u32;
        w.pkt(
            // 5-byte packet — address field spans bits [5,31] (PI-V3D-10 boot-P6 root cause #1;
            // a 4-byte emission makes the CLE eat the next opcode as the record-address MSB).
            Pkt::new(P_GL_SHADER_STATE, 5)
                .f(0, 5, num_attrs as u64)
                .f(5, 27, (shadrec >> 5) as u64)
                .done(),
        );
        w.pkt(
            Pkt::new(P_VERTEX_ARRAY_PRIMS, 10)
                .f(0, 8, V3D_PRIM_TRIANGLES)
                .f(8, 32, count as u64)
                .f(40, 32, first as u64)
                .done(),
        );
    }
    w.pkt(Pkt::new(P_FLUSH, 1).done());
    let len = w.len();
    cache::clean_range(arena_phys() + cl_off, len);
    len
}

/// A generalised M4-style render control list (main list + generic per-tile sub-list with
/// BRANCH_TO_IMPLICIT_TILE_LIST), mirroring `build_m4_rcl` with the offsets parameterised. Publishes
/// both lists; returns the MAIN list byte length (the CT1 [BA, EA) extent).
fn build_battery_rcl(rcl_off: usize, sublist_off: usize, target_off: usize) -> usize {
    let target = (arena_phys() + target_off) as u32;
    let sublist_start = (arena_phys() + sublist_off) as u32;
    let tile_alloc = (arena_phys() + OFF_BIN_TILEALLOC) as u32;
    let stride = (TARGET_W * TARGET_BPP) as u64;

    let mut s = RclWriter::new(sublist_off);
    s.pkt(Pkt::new(P_TILE_COORDINATES_IMPLICIT, 1).done());
    s.pkt(Pkt::new(P_END_OF_LOADS, 1).done());
    s.pkt(Pkt::new(P_PRIM_LIST_FORMAT, 2).f(0, 6, PRIM_TYPE_LIST_TRIANGLES).done());
    s.pkt(Pkt::new(P_BRANCH_TO_IMPLICIT_TILE_LIST, 2).f(0, 8, 0).done());
    s.pkt(
        Pkt::new(P_STORE_TILE_BUFFER_GENERAL, 13)
            .f(0, 4, 0)
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
    cache::clean_range(arena_phys() + sublist_off, sublist_len);

    let mut w = RclWriter::new(rcl_off);
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
            .f(0, 2, TILE_ALLOC_BLOCK_SIZE_128B) // PI-V3D-14: match bin config's initial block
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
    let len = w.len();
    cache::clean_range(arena_phys() + rcl_off, len);
    len
}

/// FS uniform stream for a solid colour `rgba` at `off` (unorm8 → f32 channels + the two TLB configs,
/// same FIFO order as `write_fs_uniforms`). Publishes; returns the byte length.
fn write_fs_uniforms_colour(off: usize, rgba: u32) -> usize {
    let r = ((rgba & 0xFF) as f32 / 255.0).to_bits();
    let g = (((rgba >> 8) & 0xFF) as f32 / 255.0).to_bits();
    let b = (((rgba >> 16) & 0xFF) as f32 / 255.0).to_bits();
    let a = (((rgba >> 24) & 0xFF) as f32 / 255.0).to_bits();
    let unif: [u32; 6] = [r, g, b, a, 0xFFFF_FF84, 0xFFFF_FF3F];
    for (i, w) in unif.iter().enumerate() {
        arena_write_u32(off + i * 4, *w);
    }
    cache::clean_range(arena_phys() + off, unif.len() * 4);
    unif.len() * 4
}

/// Read one 32-bit pixel from an arbitrary battery target at (x, y).
#[inline]
fn target_sample(target_off: usize, x: usize, y: usize) -> u32 {
    let off = target_off + (y * TARGET_W + x) * TARGET_BPP;
    let arena = &raw const V3D_ARENA;
    unsafe {
        let b = &(*arena).bytes;
        u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
    }
}

/// Blit a 64×64 battery target to the panel at pixel origin (x0, y0) — the same bounds-clipped volatile
/// idiom as `blit_target`/`blit_m4_target`, parameterised.
fn blit_target_at(fb: &FbTarget, target_off: usize, x0: usize, y0: usize) {
    if fb.base == 0 || fb.bytes_per_pixel < 4 {
        return;
    }
    let w = TARGET_W.min(fb.width.saturating_sub(x0));
    let h = TARGET_H.min(fb.height.saturating_sub(y0));
    for y in 0..h {
        for x in 0..w {
            let px = target_sample(target_off, x, y);
            let dst = fb.base as usize
                + (y0 + y) * fb.stride_px * fb.bytes_per_pixel
                + (x0 + x) * fb.bytes_per_pixel;
            if dst + 4 <= fb.base as usize + fb.size {
                unsafe { core::ptr::write_volatile(dst as *mut u32, px) };
            }
        }
    }
}

/// Write one vec4-f32 vertex into the arena at `off`.
#[inline]
fn write_vert4(off: usize, v: [f32; 4]) {
    for (j, c) in v.iter().enumerate() {
        arena_write_u32(off + j * 4, c.to_bits());
    }
}

/// ── M5: the GRADIENT triangle — per-vertex colour varyings through the QPU varying path. ─────────
fn m5_gradient_job(fb: Option<FbTarget>) {
    serial_println!(":: V3D: M5 gradient — per-vertex colour varyings (ldvary FS) ::");

    // Shaders: verified CS (position passthrough) for binning; the widened gradient VS + ldvary FS.
    let vs_len = write_shader_words(OFF_M5_VS_CODE, &GRAD_VS_WORDS);
    let fs_len = write_shader_words(OFF_M5_FS_CODE, &GRAD_FS_WORDS);
    cache::clean_range(arena_phys() + OFF_M5_VS_CODE, vs_len);
    cache::clean_range(arena_phys() + OFF_M5_FS_CODE, fs_len);

    // FS uniforms: alpha=1.0 then the two TLB configs (FIFO order of GRAD_FS_WORDS' pops).
    let unif: [u32; 3] = [1.0f32.to_bits(), 0xFFFF_FF84, 0xFFFF_FF3F];
    for (i, w) in unif.iter().enumerate() {
        arena_write_u32(OFF_M5_FS_UNIF + i * 4, *w);
    }
    cache::clean_range(arena_phys() + OFF_M5_FS_UNIF, unif.len() * 4);
    // VS uniforms: EIGHT VPM read-offsets (vec4 pos + vec4 colour); metal-refinement surface like the
    // 4-offset stream PI-V3D-9 flagged.
    for i in 0..8u32 {
        arena_write_u32(OFF_M5_VS_UNIF + (i as usize) * 4, i);
    }
    cache::clean_range(arena_phys() + OFF_M5_VS_UNIF, 8 * 4);

    // Interleaved vertex data: [pos vec4 | colour vec4] × 3, stride 32 B. Colours are the f32
    // decomposition of the per-vertex primaries.
    for (i, v) in TRI_VERTS.iter().enumerate() {
        write_vert4(OFF_M5_VTX + i * 32, *v);
        let c = M5_VERT_COLOURS[i];
        let col = [
            (c & 0xFF) as f32 / 255.0,
            ((c >> 8) & 0xFF) as f32 / 255.0,
            ((c >> 16) & 0xFF) as f32 / 255.0,
            1.0,
        ];
        write_vert4(OFF_M5_VTX + i * 32 + 16, col);
    }
    cache::clean_range(arena_phys() + OFF_M5_VTX, 3 * 32);

    // Shader record: CS reads only position (attr 0); VS reads position + colour (attrs 0 and 1);
    // 4 varyings (the colour vec4) flow VS → FS.
    let num_attrs = build_shader_record_at(
        OFF_M5_SHADREC,
        OFF_CS_CODE, // verified position-only coordinate shader (binning needs no colour)
        OFF_M5_VS_CODE,
        OFF_M5_FS_CODE,
        OFF_M5_FS_UNIF,
        OFF_M5_VS_UNIF,
        OFF_CS_UNIF, // the M4 CS read-offset stream (still published; position-only)
        4,
        &[(OFF_M5_VTX, 32, 4, 4), (OFF_M5_VTX + 16, 32, 0, 4)],
    );
    let bin_len = build_bin_cl_at(OFF_M5_BIN_CL, &[(OFF_M5_SHADREC, num_attrs, 0, 3)]);
    let rcl_len = build_battery_rcl(OFF_M5_RCL, OFF_M5_SUBLIST, OFF_M5_TARGET);

    fill_region(OFF_M5_TARGET, TARGET_BYTES, BAT_SENTINEL);
    cache::clean_range(arena_phys() + OFF_M5_TARGET, TARGET_BYTES);

    let job = kick_bin_render(OFF_M5_BIN_CL, bin_len, OFF_M5_RCL, rcl_len);
    cache::clean_invalidate_range(arena_phys() + OFF_M5_TARGET, TARGET_BYTES);

    // Witness: three interior samples near the three coloured corners must be pairwise DISTINCT and
    // neither clear nor sentinel (interpolation produced per-corner-dominated colours); two exterior
    // corners must be the clear colour. Colour-exactness is the flagged metal seam (raw ldvary).
    let s0 = target_sample(OFF_M5_TARGET, 16, 48); // near lower-left (red) corner
    let s1 = target_sample(OFF_M5_TARGET, 47, 48); // near lower-right (green) corner
    let s2 = target_sample(OFF_M5_TARGET, 32, 18); // near top (blue) corner
    let o0 = target_sample(OFF_M5_TARGET, 2, 2);
    let o1 = target_sample(OFF_M5_TARGET, 61, 2);
    let interior_live = |s: u32| s != CLEAR_RGBA && s != BAT_SENTINEL;
    let distinct = s0 != s1 && s1 != s2 && s0 != s2;
    let pass = job.clean()
        && distinct
        && interior_live(s0)
        && interior_live(s1)
        && interior_live(s2)
        && o0 == CLEAR_RGBA
        && o1 == CLEAR_RGBA;
    serial_println!(
        ":: V3D: M5 gradient {} — in={:#010x}/{:#010x}/{:#010x} distinct={} out={:#010x}/{:#010x} ran={}/{} idled={}/{} faults={:#x} (varying math = metal seam) ::",
        if pass { "PASS" } else { "FAIL" },
        s0, s1, s2, distinct as u32, o0, o1,
        job.bin_ran as u32, job.r_ran as u32, job.bin_idled as u32, job.r_idled as u32, job.fault
    );
    super::exceptions::serror_drain_request("v3d: M5 gradient kick window");
    if pass {
        if let Some(fb) = fb {
            blit_target_at(&fb, OFF_M5_TARGET, 2 * (TARGET_W + 8), 0); // right of the M4 blit
        }
    }
}

/// Build the shared M6/M7 solid-colour scaffold: a shader record at OFF_BAT_SHADREC (+`rec_slot`×128)
/// using the VERIFIED M4 shaders with vertex data at OFF_BAT_VTX and FS uniforms at `fs_unif_off`.
fn build_bat_solid_record(rec_slot: usize, fs_unif_off: usize) -> (usize, u32) {
    let rec_off = OFF_BAT_SHADREC + rec_slot * 128;
    let n = build_shader_record_at(
        rec_off,
        OFF_CS_CODE,
        OFF_VS_CODE,
        OFF_FS_CODE,
        fs_unif_off,
        OFF_VS_UNIF,
        OFF_CS_UNIF,
        0, // solid colour: no varyings (the M4 shape)
        &[(OFF_BAT_VTX, 16, 4, 4)],
    );
    (rec_off, n)
}

/// ── M6: the ANIMATED triangle — re-record + re-kick per frame, ~5 s of sustained bin/render. ─────
fn m6_animated_job(fb: Option<FbTarget>) {
    serial_println!(
        ":: V3D: M6 animate — {} frames @ ~{} ms (sustained bin/render loop) ::",
        M6_FRAMES, M6_FRAME_PACE_MS
    );

    // Solid-colour scaffold: verified M4 shaders (already written + published by triangle_job), one
    // record whose attribute data lives at OFF_BAT_VTX. FS uniforms reuse the amber M4 stream.
    let (rec_off, num_attrs) = build_bat_solid_record(0, OFF_FS_UNIF);
    let bin_len = build_bin_cl_at(OFF_BAT_BIN_CL, &[(rec_off, num_attrs, 0, 3)]);
    let rcl_len = build_battery_rcl(OFF_BAT_RCL, OFF_BAT_SUBLIST, OFF_BAT_TARGET);

    fill_region(OFF_BAT_TARGET, TARGET_BYTES, BAT_SENTINEL);
    cache::clean_range(arena_phys() + OFF_BAT_TARGET, TARGET_BYTES);

    let mut frames_ok = 0usize;
    let mut faults = 0u32;
    let mut fault_frames = 0usize;
    // 144 rotation frames + one final identity frame (so the closing sample-verify has a known pose).
    for frame in 0..=M6_FRAMES {
        let (c, s) = if frame == M6_FRAMES { ROT24[0] } else { ROT24[frame % ROT24.len()] };
        for (i, v) in TRI_VERTS.iter().enumerate() {
            let (x, y) = (v[0], v[1]);
            write_vert4(
                OFF_BAT_VTX + i * 16,
                [x * c - y * s, x * s + y * c, v[2], v[3]],
            );
        }
        cache::clean_range(arena_phys() + OFF_BAT_VTX, 3 * 16);
        let job = kick_bin_render(OFF_BAT_BIN_CL, bin_len, OFF_BAT_RCL, rcl_len);
        if job.clean() {
            frames_ok += 1;
        }
        if job.fault != 0 {
            faults |= job.fault;
            fault_frames += 1;
        }
        if frame < M6_FRAMES {
            settle_ms(M6_FRAME_PACE_MS); // ~5 s of wall-clock animation for the eyeball witness
        }
        // Live on-glass animation: blit each frame as it completes (the eyeball IS the witness).
        if let Some(fbt) = fb {
            cache::clean_invalidate_range(arena_phys() + OFF_BAT_TARGET, TARGET_BYTES);
            blit_target_at(&fbt, OFF_BAT_TARGET, 0, TARGET_H + 8); // below the M3 blit
        }
    }

    // Closing verify on the identity-pose final frame: centroid = triangle colour, corner = clear.
    cache::clean_invalidate_range(arena_phys() + OFF_BAT_TARGET, TARGET_BYTES);
    let centroid = target_sample(OFF_BAT_TARGET, 32, 34);
    let corner = target_sample(OFF_BAT_TARGET, 2, 2);
    let total = M6_FRAMES + 1;
    let pass = frames_ok == total && faults == 0 && centroid == TRI_RGBA && corner == CLEAR_RGBA;
    serial_println!(
        ":: V3D: M6 animate {} — frames={}/{} faults={:#x} (fault-frames={}) centroid={:#010x} corner={:#010x} ::",
        if pass { "PASS" } else { "FAIL" },
        frames_ok, total, faults, fault_frames, centroid, corner
    );
    super::exceptions::serror_drain_request("v3d: M6 animate kick window");
}

/// ── M7: the MULTI-PRIMITIVE frame — a 12-wedge pinwheel in four colours (four draws, one frame). ─
fn m7_multiprim_job(fb: Option<FbTarget>) {
    serial_println!(":: V3D: M7 multiprim — 12-triangle pinwheel, 4 colour draws ::");

    // Vertex data: wedge k = centre, rim(θk), rim(θk+30°); θk = k·30° (every other ROT24 entry).
    const R: f32 = 0.8;
    for k in 0..12 {
        let (c0, s0) = ROT24[(2 * k) % 24];
        let (c1, s1) = ROT24[(2 * k + 2) % 24];
        let base = OFF_BAT_VTX + k * 3 * 16;
        write_vert4(base, [0.0, 0.0, 0.5, 1.0]);
        write_vert4(base + 16, [R * c0, R * s0, 0.5, 1.0]);
        write_vert4(base + 32, [R * c1, R * s1, 0.5, 1.0]);
    }
    cache::clean_range(arena_phys() + OFF_BAT_VTX, 12 * 3 * 16);

    // Four draws: 3 consecutive wedges each, distinct FS uniform stream (solid colour per group) —
    // multi-colour without any new QPU words: the verified FS reads its colour from the uniform FIFO.
    let mut draws: [(usize, u32, u32, u32); 4] = [(0, 0, 0, 0); 4];
    for (k, &colour) in M7_COLOURS.iter().enumerate() {
        let unif_off = OFF_M7_UNIF + k * 64;
        write_fs_uniforms_colour(unif_off, colour);
        let (rec_off, n) = build_bat_solid_record(k, unif_off);
        draws[k] = (rec_off, n, (k * 9) as u32, 9);
    }
    let bin_len = build_bin_cl_at(OFF_BAT_BIN_CL, &draws);
    let rcl_len = build_battery_rcl(OFF_BAT_RCL, OFF_BAT_SUBLIST, OFF_BAT_TARGET);

    fill_region(OFF_BAT_TARGET, TARGET_BYTES, BAT_SENTINEL);
    cache::clean_range(arena_phys() + OFF_BAT_TARGET, TARGET_BYTES);

    let job = kick_bin_render(OFF_BAT_BIN_CL, bin_len, OFF_BAT_RCL, rcl_len);
    cache::clean_invalidate_range(arena_phys() + OFF_BAT_TARGET, TARGET_BYTES);

    // Witness: one sample inside each colour group's mid-wedge (live: neither clear nor sentinel —
    // the exact colour-to-quadrant mapping depends on the viewport transform, the flagged PI-V3D-9
    // metal seam) + two rim corners the R=0.8 pinwheel cannot reach = clear.
    let q: [(usize, usize); 4] = [(41, 25), (23, 25), (23, 43), (41, 43)];
    let mut lives = 0u32;
    let mut vals = [0u32; 4];
    for (i, &(x, y)) in q.iter().enumerate() {
        let s = target_sample(OFF_BAT_TARGET, x, y);
        vals[i] = s;
        if s != CLEAR_RGBA && s != BAT_SENTINEL {
            lives += 1;
        }
    }
    let o0 = target_sample(OFF_BAT_TARGET, 1, 1);
    let o1 = target_sample(OFF_BAT_TARGET, 62, 62);
    let pass = job.clean() && lives == 4 && o0 == CLEAR_RGBA && o1 == CLEAR_RGBA;
    serial_println!(
        ":: V3D: M7 multiprim {} — quads={:#010x}/{:#010x}/{:#010x}/{:#010x} live={}/4 out={:#010x}/{:#010x} ran={}/{} idled={}/{} faults={:#x} ::",
        if pass { "PASS" } else { "FAIL" },
        vals[0], vals[1], vals[2], vals[3], lives, o0, o1,
        job.bin_ran as u32, job.r_ran as u32, job.bin_idled as u32, job.r_idled as u32, job.fault
    );
    super::exceptions::serror_drain_request("v3d: M7 multiprim kick window");
    if pass {
        if let Some(fbt) = fb {
            blit_target_at(&fbt, OFF_BAT_TARGET, TARGET_W + 8, TARGET_H + 8);
        }
    }
}

/// ── M8: BLIT TO SCANOUT — composite the battery render target onto the live framebuffer console and
/// read the written words back from the panel memory (end-to-end GPU→glass witness). The blit is a
/// bounded 64×64 region (the GUI stays usable); readback compares three probe pixels source↔panel. ─
fn m8_blit_scanout(fb: Option<FbTarget>) {
    let Some(fbt) = fb else {
        serial_println!(":: V3D: M8 blit SKIP — no framebuffer target (serial-only run) ::");
        return;
    };
    if fbt.base == 0 || fbt.bytes_per_pixel < 4 {
        serial_println!(":: V3D: M8 blit SKIP — framebuffer not blittable (bpp<4 or null base) ::");
        return;
    }
    // Composite the M7 scene (the battery target's final contents) at a fixed console-corner slot.
    let (x0, y0) = (2 * (TARGET_W + 8), TARGET_H + 8);
    blit_target_at(&fbt, OFF_BAT_TARGET, x0, y0);

    // Readback witness: three probe pixels re-read VOLATILE from the panel memory must equal the
    // source target words (proves the composite landed in scanout-visible memory, not a stale cache).
    let probes: [(usize, usize); 3] = [(0, 0), (32, 34), (63, 63)];
    let mut ok = 0u32;
    let mut got = [0u32; 3];
    let mut want = [0u32; 3];
    for (i, &(x, y)) in probes.iter().enumerate() {
        want[i] = target_sample(OFF_BAT_TARGET, x, y);
        let dst = fbt.base as usize
            + (y0 + y) * fbt.stride_px * fbt.bytes_per_pixel
            + (x0 + x) * fbt.bytes_per_pixel;
        if x0 + x < fbt.width && y0 + y < fbt.height && dst + 4 <= fbt.base as usize + fbt.size {
            got[i] = unsafe { core::ptr::read_volatile(dst as *const u32) };
            if got[i] == want[i] {
                ok += 1;
            }
        }
    }
    let pass = ok == probes.len() as u32;
    serial_println!(
        ":: V3D: M8 blit {} — probes {}/{} panel={:#010x}/{:#010x}/{:#010x} src={:#010x}/{:#010x}/{:#010x} @({},{}) ::",
        if pass { "PASS" } else { "FAIL" },
        ok, probes.len(), got[0], got[1], got[2], want[0], want[1], want[2], x0, y0
    );
}

/// PI-V3D-11 battery entry: run the four visible stages in order. Called from `bringup` AFTER the M3
/// clear + M4 triangle regressions; only reachable on metal (QEMU returned at BLOCK-DOWN). Each stage
/// is independent — a FAIL prints its verdict and the battery continues (every stage is a witness the
/// attended sitting wants regardless of the others).
fn battery(fb: Option<FbTarget>) {
    serial_println!(":: V3D: PI-V3D-11 battery — M5 gradient, M6 animate, M7 multiprim, M8 blit ::");
    m5_gradient_job(fb);
    m6_animated_job(fb);
    m7_multiprim_job(fb);
    m8_blit_scanout(fb);
    super::exceptions::serror_drain_request("v3d: battery exit");
}

/// The number of VISIBLE battery stages `battery` replays (M5 gradient, M6 animate, M7 multiprim,
/// M8 blit). Kept as one constant so the `v3d` app's `stages=N` witness never drifts from `battery`.
const VISIBLE_BATTERY_STAGES: u32 = 4;

// ── PI-APP-1 replay state. Latched once at the tail of a successful boot `bringup` (block up, MMU
// programmed, visible battery already run). The `v3d` shell app reads it to REPLAY the visible stages
// on the live framebuffer WITHOUT re-entering the init path. `V3D_REPLAY_FB` is written exactly once
// (single-threaded boot, pre-shell) and only ever read after `V3D_REPLAY_READY` is observed true, so
// the plain `static mut` needs no further synchronisation beyond the acquire/release on the flag.
static V3D_REPLAY_READY: AtomicBool = AtomicBool::new(false);
static mut V3D_REPLAY_FB: Option<FbTarget> = None;

/// PI-APP-1: replay the VISIBLE V3D battery on the live framebuffer, on demand from the shell.
///
/// Re-entry safety: this does NOT call `bringup`. It reuses the state boot already established — the
/// V3D power domain, clock gate, PM/ASB bridges and the V3D MMU all stay enabled from boot, and the
/// buffer arena stays identity-mapped. Each visible stage (`m5..m8`) rebuilds its own control list
/// into fixed arena offsets from scratch and re-kicks the GPU, so re-running them is idempotent — no
/// static needs re-init and no init step is duplicated. If boot never brought the block up (QEMU
/// raspi4b returns at BLOCK-DOWN; any fail-closed probe/MMU verdict), the flag is false and we print
/// a skip witness and touch no MMIO — the serial-only gate stays clean.
///
/// Prints `:: V3D: app replay start ::` / `:: V3D: app replay done (stages=N) ::` for the bench.
/// Returns the number of stages replayed (0 when the block was never up).
pub fn run_visible_battery_again() -> u32 {
    serial_println!(":: V3D: app replay start ::");
    if !V3D_REPLAY_READY.load(Ordering::Acquire) {
        serial_println!(
            ":: V3D: app replay done (stages=0) — V3D not brought up this boot (absent/fail-closed); nothing to replay ::"
        );
        return 0;
    }
    // SAFETY: written once at boot before any shell exists; only read here after the acquire load
    // above observed the release store, so the FbTarget is fully published.
    let fb = unsafe { V3D_REPLAY_FB };
    battery(fb);
    serial_println!(
        ":: V3D: app replay done (stages={}) ::",
        VISIBLE_BATTERY_STAGES
    );
    VISIBLE_BATTERY_STAGES
}
