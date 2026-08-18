// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Bare-metal Raspberry Pi 4 boot (the `baremetal` feature). The VideoCore GPU ROM reads the microSD
// slot perfectly (it loads start4.elf/RPI_EFI.fd from it) — it's only EDK2/UEFI's own SD driver that
// can't, which is why the UEFI path needs USB. So here we cut UEFI out: the GPU ROM loads our flat
// `kernel8.img` directly to 0x80000 and jumps to `_start` (in main.rs's .text.boot) at EL2 with x0 =
// DTB, MMU off, caches off. `_start` (asm) parks secondary cores, zeroes BSS, sets the stack, and
// calls `__rust_boot`, which calls the two functions here — `mmu_init` then `build_boot_info` — and
// then enters the normal `kernel_main`.
//
// Why the MMU is mandatory (not just nice): the GPU hands off with the MMU OFF, where all memory is
// treated as Device/Strongly-ordered. AArch64 exclusive accesses (`ldxr`/`stxr`) — which every
// spinlock and atomic in this kernel rely on — are CONSTRAINED UNPREDICTABLE on non-Normal-cacheable
// memory. So before any of the kernel's locks run we must enable the MMU with at least RAM mapped
// Normal cacheable. We use the simplest possible map: a single L1 table of 1 GiB identity blocks.
//
// EL: the firmware hands every core off at EL2 (the hypervisor level — incidental, not for isolation).
// A normal OS runs at EL1, so each core calls `drop_to_el1` (below) BEFORE `mmu_init`/`enable_mmu`; the
// MMU and everything after it therefore run in the EL1&0 translation regime (TTBR0_EL1/TCR_EL1/
// SCTLR_EL1). The drop is purely additive — it configures the EL1-facing controls that would otherwise
// trap or read UNKNOWN, and leaves the (now-unused-for-translation) EL2 regime alone.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use unaos_boot_info::{BootInfo, FrameBufferInfo, MemoryRegion, MemoryRegionKind, PixelFormat};

/// A single Level-1 translation table: 512 entries × 8 bytes = one 4 KiB page. With a 4 KiB granule
/// and a 39-bit VA (TCR T0SZ=25) the top lookup level is L1 and each entry maps a 1 GiB block, so
/// these 512 entries cover the whole 512 GiB VA — we only fill the first four (0–4 GiB). Lives in
/// BSS (zeroed by `_start` before we fill it).
#[repr(C, align(4096))]
struct PageTable([u64; 512]);
static mut L1: PageTable = PageTable([0; 512]);

/// Second- and third-level tables for the EL0 user window (M6a). To grant EL0 access at 4 KiB
/// granularity — not the 2 MiB or 1 GiB of a block — the kernel's L1[0] (0–1 GiB) is demoted from a
/// 1 GiB block to a table: L1[0] → L2_USER (512 × 2 MiB) → (for the one 2 MiB block containing
/// `USER_REGION`) L3_USER (512 × 4 KiB). ONLY the pages backing `USER_REGION` are EL0-accessible;
/// every other page of 0–1 GiB stays EL1-only, exactly as the old 1 GiB block was. Both in BSS.
static mut L2_USER: PageTable = PageTable([0; 512]);
static mut L3_USER: PageTable = PageTable([0; 512]);

/// The EL0 user window, identity-mapped (VA == PA): the user program is copied to the bottom (the
/// CODE page) and the user stack grows down from the top (the DATA pages). 16 KiB in BSS (zeroed by
/// `_start`), aligned to its own SIZE so it can never straddle a 16 KiB — a fortiori 2 MiB —
/// boundary: all four pages are structurally guaranteed to fall inside the single L3_USER-covered
/// block, whatever BSS layout future milestones produce. Deliberately small — only these pages are
/// ever exposed to EL0.
///
/// M6b permission split: page 0 = CODE (EL0-RX/EL1-RO after `protect_user_code`; RW during the blob
/// copy), pages 1–3 = DATA/STACK (EL0+EL1 RW, never executable at any EL).
pub const USER_REGION_SIZE: usize = 0x4000; // 16 KiB = 4 pages
/// The CODE page(s) at the bottom of USER_REGION — the only EL0-executable memory in the system.
pub const USER_CODE_SIZE: usize = 0x1000;

// ---------------------------------------------------------------------------------------------
// ELF-3: the per-process off-screen framebuffer surface (SYS_FB_MAP).
//
// A dedicated kernel-allocated surface each EL0 process may map (EL0-RW, Normal-cacheable) plus a
// read-only info page (geometry). It lives in the RESERVED VA HOLE immediately ABOVE the 16 KiB EL0
// program window — the backing is carried per-slot (see `SLOT_BACKING`), and each slot's L3 maps it
// only after `map_slot_fb` is called (from SYS_FB_MAP). EL0 NEVER gets the real scan-out: the kernel
// composites the surface to the screen through SYS_FB_PRESENT (a public present hook the video
// subsystem registers). Small (a 32×32 ARGB8888 surface = one page) — the point is the mechanism, not
// resolution.
//
// WC-B: the hole now carries N WINDOW SURFACE SLOTS, not one surface. Layout in the hole:
//   [+0x4000]                             RO info page (1 page)
//   [+0x5000 + w * FB_WIN_SLOT_SIZE]      window `w`'s RW surface slot (16 pages), w in 0..FB_WIN_SLOTS
// Window slot 0 begins at exactly the VA the single ELF-3 surface used to occupy, so `fb_surface_va()`
// (what `SYS_FB_MAP` returns) is BYTE-IDENTICAL to before and the existing VUG.ELF binary is unaffected.
// Each slot is 64 KiB = 16 pages, which covers the largest surface this arc admits (128×128 ARGB8888 =
// 65536 B). A surface is negotiated at map time and only its PAGE-MULTIPLE size is actually mapped — the
// rest of the slot stays at its reserved (EL1-only identity) leaf, so nothing beyond the negotiated
// surface is reachable from EL0. Every mapped surface page uses `user_data_page` — the SAME MMU
// attributes (EL0+EL1 RW, UXN, Normal-cacheable, nG) as the single ELF-3 surface page had.
pub const FB_INFO_SIZE: usize = 0x1000; // the read-only geometry page (1 page)
pub const FB_SURFACE_W: u32 = 32;
pub const FB_SURFACE_H: u32 = 32;
pub const FB_SURFACE_STRIDE: u32 = FB_SURFACE_W * 4; // ARGB8888, 4 bytes/pixel
pub const FB_SURFACE_SIZE: usize = (FB_SURFACE_STRIDE * FB_SURFACE_H) as usize; // 4096 = 1 page
/// WC-B: window surface slots per process address space. Matches the compositor's fixed window table
/// size (STOP tripwire: like `USER_SLOTS` this cap is deliberate — do not raise it for a demo).
pub const FB_WIN_SLOTS: usize = 8;
/// WC-B: bytes of VA reserved per window surface slot — 64 KiB = 16 pages = a 128×128 ARGB8888 surface.
pub const FB_WIN_SLOT_SIZE: usize = 0x1_0000;
/// WC-B: the largest surface edge a window may negotiate (128×128×4 == `FB_WIN_SLOT_SIZE`).
pub const FB_WIN_MAX_W: u32 = 128;
pub const FB_WIN_MAX_H: u32 = 128;
/// The FB info + window-surface VA hole reserved above the program window.
pub const FB_REGION_SIZE: usize = FB_INFO_SIZE + FB_WIN_SLOTS * FB_WIN_SLOT_SIZE; // 0x81000

/// Total per-slot reserved region: the 16 KiB EL0 program window + the FB info/window-surface hole.
/// The VA ANCHOR (`USER_REGION`) is aligned to 0x100000 (>= the 0x85000 size) so the whole region is
/// STRUCTURALLY guaranteed to fall inside one 2 MiB L3_USER block (0x100000 divides 2 MiB, and the size
/// is <= 0x100000, so it can never straddle a 2 MiB boundary) — the single per-slot L3 covers it all.
const USER_STATIC_SIZE: usize = USER_REGION_SIZE + FB_REGION_SIZE; // 0x85000
#[repr(C, align(0x100000))]
struct UserRegion([u8; USER_STATIC_SIZE]);
static mut USER_REGION: UserRegion = UserRegion([0; USER_STATIC_SIZE]);

/// The per-slot BACKING frames. Deliberately a distinct type from the VA anchor: only the anchor needs
/// the 1 MiB alignment (it defines the VA hole and must not straddle 2 MiB). A backing is consumed one
/// PAGE at a time (`build_slot`/`map_slot_fb*` install one leaf per page), so page alignment is the real
/// requirement — and `USER_STATIC_SIZE` is itself a page multiple, so the array stride keeps every slot
/// page-aligned. Using the anchor's alignment here would burn ~0.5 MiB of BSS per slot for nothing.
#[repr(C, align(0x1000))]
struct SlotBacking([u8; USER_STATIC_SIZE]);

// --- Translation attributes (ARM ARM DDI0487). We drop to EL1 (see `drop_to_el1`) before enabling
// the MMU, so these program the EL1&0 regime (TTBR0_EL1/TCR_EL1/MAIR_EL1/SCTLR_EL1). ---
// MAIR: AttrIdx 0 = Normal Inner/Outer Write-Back non-transient (0xFF); AttrIdx 1 = Device-nGnRnE
// (0x00). Regime-independent — same layout at EL1 and EL2.
const MAIR_VAL: u64 = 0xFF;

// TCR_EL1 (NOT TCR_EL2 — the field layout differs). T0SZ=25 (39-bit VA → L1 top level, 1 GiB blocks),
// IRGN0=ORGN0=WB, SH0=Inner-shareable, TG0=4 KiB. The TTBR1 (high) half is unused, so EPD1=1 disables
// its table walk (TTBR1_EL1 stays 0); T1SZ/TG1 are given legal values regardless. Note the two traps
// for anyone copying TCR_EL2: PS is IPS at bits [34:32] (not [18:16]), and bits 23/31 are EPD1/TG1
// here, NOT the RES1 bits the non-VHE TCR_EL2 short format has.
const TCR_EL1_VAL: u64 = 25            // T0SZ  [5:0]
    | (0b01 << 8)                      // IRGN0 = WB
    | (0b01 << 10)                     // ORGN0 = WB
    | (0b11 << 12)                     // SH0   = inner shareable
    | (0b00 << 14)                     // TG0   = 4 KiB
    | (25 << 16)                       // T1SZ  [21:16] (TTBR1 unused; legal value)
    | (1 << 23)                        // EPD1  = disable the TTBR1 table walk
    | (0b10 << 30)                     // TG1   = 4 KiB (legal encoding; TTBR1 unused)
    | (0b001 << 32);                   // IPS   = 36-bit / 64 GiB, at [34:32]

// SCTLR_EL1 built as an ABSOLUTE value, not read-modify-write. SCTLR_EL1 resets to an architecturally
// UNKNOWN value (unlike SCTLR_EL2, which firmware initialised before handing off — which is why the
// old EL2 path could RMW it safely). An RMW here could read the RES1 bits as 0 and leave them cleared
// → CONSTRAINED UNPREDICTABLE translation/execution. So OR the Armv8.0 Cortex-A72 SCTLR_EL1 RES1 mask
// (0x30D00800 = bits 29,28,23,22,20,11) with M (MMU), C (data cache), I (instruction cache).
const SCTLR_EL1_VAL: u64 = 0x30D0_0800 | (1 << 0) | (1 << 2) | (1 << 12);

// L1 block descriptor lower attributes: bits[1:0]=0b01 (block), AttrIndx[4:2], AP[7:6]=0b00
// (privileged RW, no EL0 access — the EL1&0 regime), SH[9:8], AF=bit10.
const DESC_BLOCK: u64 = 0b01;
const DESC_AF: u64 = 1 << 10;
const SH_INNER: u64 = 0b11 << 8;
const ATTR_NORMAL: u64 = 0 << 2; // AttrIdx 0
const ATTR_DEVICE: u64 = 1 << 2; // AttrIdx 1
// Execute-never. The EL1&0 translation regime splits execute-never into UXN (bit 54, EL0) and PXN
// (bit 53, EL1); a Device/peripheral block must set PXN to be non-executable by the EL1 kernel (the
// old EL2 regime had only bit 54 = XN). Set both so peripheral memory is never executable at any EL.
const DESC_XN: u64 = (1 << 54) | (1 << 53);

/// Normal, cacheable, inner-shareable, executable block (RAM where the kernel/heap/stack live).
const fn ram_block(pa: u64) -> u64 {
    pa | DESC_AF | SH_INNER | ATTR_NORMAL | DESC_BLOCK
}
/// Device-nGnRnE, execute-never block (the peripheral window: PL011 0xFE201000, GIC 0xFF841000,
/// mailbox 0xFE00B880 all sit in the 0xC000_0000–0xFFFF_FFFF GiB).
const fn device_block(pa: u64) -> u64 {
    pa | DESC_XN | DESC_AF | ATTR_DEVICE | DESC_BLOCK
}

// --- EL0 user-window descriptor bits (M6a/M6b). ---
const DESC_TABLE: u64 = 0b11; // L1/L2 table descriptor (points at the next-level table)
const DESC_PAGE: u64 = 0b11; // L3 page descriptor — note 0b01 is INVALID at L3, unlike a block
const AP_EL0: u64 = 1 << 6; // AP[7:6]=0b01 → EL0+EL1 read-write (AP=0b00 is EL1-only)
const AP_RO_ALL: u64 = 0b11 << 6; // AP[7:6]=0b11 → read-only at BOTH EL1 and EL0
const DESC_PXN: u64 = 1 << 53; // privileged execute-never (EL1 can't execute the page)
const DESC_UXN: u64 = 1 << 54; // unprivileged execute-never (EL0 can't execute the page)
// nG (not-Global), descriptor bit 11. A leaf with nG=1 is tagged by the active ASID (TTBR0.ASID); one
// with nG=0 is Global — it matches in EVERY ASID. M6d makes ALL user-window leaves nG (the shared window
// becomes the ASID-0 boot context; each per-task slot is ASID 1..8) so the SAME user VA can map DIFFERENT
// frames per task with no same-VA global+non-global TLB conflict (which is CONSTRAINED UNPREDICTABLE).
// Kernel-only leaves (ram_page/ram_block/device_block) stay Global — ASID-agnostic, so a TTBR0/ASID switch
// needs no kernel-mapping flush and no per-slot duplication.
const DESC_NG: u64 = 1 << 11;

/// L3 4 KiB page, Normal cacheable, EL1-only (AP=0b00), executable at EL1 — the default identity page.
const fn ram_page(pa: u64) -> u64 {
    pa | DESC_AF | SH_INNER | ATTR_NORMAL | DESC_PAGE
}
/// L3 4 KiB USER DATA/STACK page: Normal cacheable, EL0+EL1 read-write (AP=0b01), never executable
/// at any EL (UXN=1, PXN=1), **non-global (nG=1)** so it is ASID-tagged (M6d). The build_l1 state of
/// ALL user pages — the CODE page starts this way too so `syscall::setup` can copy the program in at
/// EL1; `protect_user_code` then flips it.
const fn user_data_page(pa: u64) -> u64 {
    pa | DESC_NG | DESC_UXN | DESC_PXN | AP_EL0 | DESC_AF | SH_INNER | ATTR_NORMAL | DESC_PAGE
}
/// L3 4 KiB USER CODE page (M6b, the final shape written by `protect_user_code`): Normal cacheable,
/// read-only at BOTH ELs (AP=0b11), EL0-executable (UXN=0), EL1-non-executable (PXN=1). EL0 can run
/// but not modify its program; EL1 can read it (sys_write's message bytes — fine on the PAN-less
/// A72) but a kernel write is now a Current-EL permission fault, so any future loader (M6c) must
/// re-open the page before writing. **Non-global (nG=1)** so it is ASID-tagged (M6d).
const fn user_code_page(pa: u64) -> u64 {
    pa | DESC_NG | DESC_PXN | AP_RO_ALL | DESC_AF | SH_INNER | ATTR_NORMAL | DESC_PAGE
}
/// L3 4 KiB USER READ-ONLY DATA page (ELF-3 FB info page): Normal cacheable, read-only at BOTH ELs
/// (AP=0b11), never executable at either EL (UXN|PXN), non-global. EL0 can read it; the kernel writes
/// the geometry through the IDENTITY backing pointer (EL1-RW under the plain RAM mapping), never this
/// EL0 VA — so EL0 can never mutate the info page.
const fn user_ro_page(pa: u64) -> u64 {
    pa | DESC_NG | DESC_UXN | DESC_PXN | AP_RO_ALL | DESC_AF | SH_INNER | ATTR_NORMAL | DESC_PAGE
}
/// A table descriptor pointing at the next-level table. Mask to bits[47:12] so no stray bits land in
/// the table-attribute fields [63:59] (NSTable/APTable/UXNTable/PXNTable); leaving them 0 adds no
/// restriction at this level, so the leaf page's own AP/XN govern.
const fn table_desc(table_pa: u64) -> u64 {
    (table_pa & 0x0000_FFFF_FFFF_F000) | DESC_TABLE
}

/// The EL0 user window as (base PA == VA, size in bytes). The M6a routine is copied to the base and
/// run at EL0 (identity map); the caller sets SP_EL0 to the 16-aligned top. Baremetal-only.
pub fn user_region() -> (u64, usize) {
    (&raw const USER_REGION as u64, USER_REGION_SIZE)
}

/// Build the identity map and turn the MMU on. Called (with the MMU still off, at EL1 after
/// `drop_to_el1`) from `__rust_boot` after BSS has been zeroed. Plain volatile writes + system-
/// register moves only — no atomics, no locks, nothing that needs the MMU we're about to enable.
pub unsafe fn mmu_init() {
    build_l1();
    unsafe { enable_mmu() };
}

/// Populate the translation tables. BSP-only, MMU OFF (before `enable_mmu` and before the secondaries
/// are released), so the secondaries reuse the finished tables and no TLB/cache publication is needed
/// (the `tlbi vmalle1` in `enable_mmu` covers each core's cold start). Builds the EL0 user window as a
/// 3-level walk under L1[0] so EL0 permission is carved at 4 KiB granularity (see `L2_USER`/`L3_USER`).
fn build_l1() {
    let user_pa = &raw const USER_REGION as u64;
    // USER_REGION must be 4 KiB-aligned (whole L3 pages) and in the first 1 GiB (under L1[0]).
    debug_assert!(user_pa & 0xFFF == 0, "USER_REGION not 4 KiB aligned");
    debug_assert!(user_pa >> 30 == 0, "USER_REGION not in the first 1 GiB");
    // Structurally guaranteed by `UserRegion`'s align(0x100000) >= the region size; documents that every page falls
    // inside the ONE 2 MiB block L3_USER covers (a straddling tail would silently stay EL1-only).
    debug_assert!(
        user_pa >> 21 == (user_pa + USER_STATIC_SIZE as u64 - 1) >> 21,
        "USER_REGION (incl. the FB hole) straddles a 2 MiB block"
    );
    let l2_idx = (user_pa >> 21) as usize; // the 2 MiB block of 0–1 GiB that holds USER_REGION
    let l3_base = (l2_idx as u64) << 21; // that block's base PA
    let user_end = user_pa + USER_REGION_SIZE as u64;

    unsafe {
        // L3_USER: 512 × 4 KiB pages tiling [l3_base, l3_base + 2 MiB). Pages inside USER_REGION are
        // EL0-accessible; every other page stays EL1-only (identical to the old 1 GiB RAM block).
        // ALL user pages start as data pages (EL0+EL1 RW, XN) — the CODE page must be EL1-writable
        // for the blob copy; `protect_user_code` flips it to EL0-RX/EL1-RO afterwards.
        let l3 = &raw mut L3_USER as *mut u64;
        for j in 0..512 {
            let pa = l3_base | ((j as u64) << 12);
            let desc =
                if pa >= user_pa && pa < user_end { user_data_page(pa) } else { ram_page(pa) };
            l3.add(j).write_volatile(desc);
        }
        // L2_USER: 512 × 2 MiB blocks tiling [0, 1 GiB). Block `l2_idx` points at L3_USER; the other
        // 511 are plain RAM blocks (same attributes as the old 1 GiB block, just at 2 MiB).
        let l2 = &raw mut L2_USER as *mut u64;
        for i in 0..512 {
            let desc = if i == l2_idx {
                table_desc(&raw const L3_USER as u64)
            } else {
                ram_block((i as u64) << 21)
            };
            l2.add(i).write_volatile(desc);
        }
        // L1: [0] → L2_USER (0–1 GiB, now paged for the EL0 window); [1],[2] plain 1 GiB RAM; [3] the
        // Device peripheral window. (Was: L1[0] a 1 GiB RAM block.)
        let l1 = &raw mut L1 as *mut u64;
        l1.add(0).write_volatile(table_desc(&raw const L2_USER as u64));
        l1.add(1).write_volatile(ram_block(0x4000_0000));
        l1.add(2).write_volatile(ram_block(0x8000_0000));
        l1.add(3).write_volatile(device_block(0xC000_0000));
    }
}

/// Point this core's TTBR0_EL1 at the (already-built) L1 table and turn the MMU + caches on. Run on
/// EACH core with its MMU still off, AFTER `drop_to_el1` has put it at EL1 — the BSP after `build_l1`,
/// every secondary after the spin-table release (`arch::aarch64::smp`). System-register moves only;
/// no atomics/locks (the very thing the MMU is being enabled to make sound). The table is shared, so
/// no per-core state here.
pub unsafe fn enable_mmu() {
    let ttbr0 = &raw const L1 as u64;
    unsafe {
        core::arch::asm!(
            "msr MAIR_EL1, {mair}",
            "msr TCR_EL1,  {tcr}",
            "msr TTBR0_EL1, {ttbr}",
            "msr TTBR1_EL1, xzr", // high half unused (EPD1=1 disables its walk); zero it defensively
            "tlbi vmalle1",       // drop any stale EL1&0 TLB entries before translation is enabled
            "dsb sy",
            "isb",
            // Enable MMU (M=bit0), data cache (C=bit2), instruction cache (I=bit12) via an ABSOLUTE
            // SCTLR_EL1 write (SCTLR_EL1 resets UNKNOWN, so no RMW — SCTLR_EL1_VAL carries the A72
            // RES1 bits). See the const's comment.
            "msr SCTLR_EL1, {sctlr}",
            "isb",
            mair = in(reg) MAIR_VAL,
            tcr = in(reg) TCR_EL1_VAL,
            ttbr = in(reg) ttbr0,
            sctlr = in(reg) SCTLR_EL1_VAL,
            options(nostack, preserves_flags),
        );
    }
}

/// M6b: flip the user CODE pages [va, va + len) from their build_l1 state (EL0+EL1 RW, XN — so the
/// blob could be copied in) to their final EL0-RX / EL1-RO shape (`user_code_page`), and make the
/// change visible to all four cores. This is the kernel's FIRST live page-table update. The
/// descriptor rewrite is a permission-only change on a valid same-size leaf (same output address,
/// attributes, shareability), which DDI0487 (D8.16 "break-before-make") exempts from BBM — a
/// concurrent walk may transiently use either permission set, harmless because no EL0 task exists
/// yet (the caller protects strictly before the first `spawn_user`). The maintenance sequence is
/// the canonical set_pte one (DDI0487 D8.13/D8.14; the walker reads descriptors cache-coherently
/// under TCR IRGN0/ORGN0=WB, so no DC CVAC is needed):
///   dsb ishst              descriptor stores visible to every Inner-Shareable table walker
///   tlbi vaae1is (per pg)  broadcast-invalidate the VA for ALL ASIDs. VAAE1IS is all-ASID, so it drops
///                          BOTH global and non-global entries for the VA — since M6d the shared code
///                          page is nG (ASID-0-tagged), and VAAE1IS still covers it (all-ASID). The
///                          Xt operand is VA[55:12] in bits [43:0] (i.e. `va >> 12`, NOT a byte or PA),
///                          upper bits RES0 on Armv8.0 (no FEAT_TTL).
///   dsb ish                the invalidate has completed on every core in the domain
///   isb                    this core refetches translation state
///
/// Returns the AT-probe verdicts `(el0_read_ok, el1_write_denied)` taken through this core's
/// post-TLBI translation state: `at s1e0r` must translate (AP=0b11 grants EL0 read) and `at s1e1w`
/// must fault (PAR_EL1.F=1 — the EL1 write permission the blob copy used is now gone). The calling
/// BSP is the one core that deterministically walked these pages pre-flip (the copy), so a stale RW
/// TLB entry here is exactly what a broken TLBI leaves. AT is architecturally allowed to re-walk
/// instead of consulting the TLB, so a good probe is best-effort evidence — the demo's TLB-warmed
/// test core is the deterministic detector; a bad probe is always a loud, real failure.
pub unsafe fn protect_user_code(va: u64, len: usize) -> (bool, bool) {
    // STOP TRIPWIRE (per-slot tables freeze the boot kernel mappings at COPY time): this edits the SHARED
    // L3_USER in place — the table each slot COPIED at build time (see `build_slot`), so a slot does NOT
    // observe this flip on its own copy (per-slot code pages are protected separately via
    // `protect_user_slot_code`). More generally: any post-boot edit to a KERNEL mapping must be mirrored into
    // all live slot L1/L2/L3 copies (or force a slot rebuild + TLBI) — per-slot tables freeze the boot kernel
    // mappings at copy time. This routine touches only USER leaves (in-lane), so no mirroring is owed today.
    let user_pa = &raw const USER_REGION as u64;
    debug_assert!(va & 0xFFF == 0, "protect_user_code: unaligned va");
    debug_assert!(
        va >= user_pa && va + len as u64 <= user_pa + USER_REGION_SIZE as u64,
        "protect_user_code: range outside USER_REGION"
    );
    unsafe {
        let l3 = &raw mut L3_USER as *mut u64;
        let mut page = va;
        while page < va + len as u64 {
            l3.add(((page >> 12) & 0x1FF) as usize).write_volatile(user_code_page(page));
            page += 0x1000;
        }
        core::arch::asm!("dsb ishst", options(nostack, preserves_flags));
        let mut page = va;
        while page < va + len as u64 {
            core::arch::asm!("tlbi vaae1is, {}", in(reg) (page >> 12), options(nostack, preserves_flags));
            page += 0x1000;
        }
        core::arch::asm!("dsb ish", "isb", options(nostack, preserves_flags));

        // PAR_EL1 is per-core state clobbered by any AT instruction; the BSP runs this with IRQs
        // live (250 Hz on metal), so mask across each at->mrs pair rather than resting the probe
        // on a "no interrupt path ever executes AT" invariant nobody enforces.
        let (par_r, par_w, daif): (u64, u64, u64);
        core::arch::asm!(
            "mrs {daif}, DAIF",
            "msr DAIFSet, #2",
            "at s1e0r, {va}",
            "isb",
            "mrs {par_r}, PAR_EL1",
            "at s1e1w, {va}",
            "isb",
            "mrs {par_w}, PAR_EL1",
            "msr DAIF, {daif}",
            va = in(reg) va,
            par_r = out(reg) par_r,
            par_w = out(reg) par_w,
            daif = out(reg) daif,
            options(nostack, preserves_flags),
        );
        ((par_r & 1) == 0, (par_w & 1) == 1)
    }
}

// =============================================================================================
// M6d: per-task address spaces (ASIDs) + per-task user stacks
// =============================================================================================
//
// Through M6c each EL0 task shared the ONE `USER_REGION` window mapped by the single static chain
// `L1 -> L2_USER -> L3_USER`. That is safe only while no EL0 program writes its stack. M6d gives each
// task its OWN translation-table branch and its OWN 16 KiB backing at the SAME virtual addresses, tagged
// by a distinct ASID, so a task can write its stack without disturbing any other and a task switch needs
// no TLB flush (ASID-tagged non-global entries coexist; global kernel entries are ASID-agnostic).
//
// Layout: 8 slots (a deliberate cap — STOP if a demo needs more), each a private L1/L2/L3 + a 16 KiB
// backing. ASID = slot + 1 (1..=8); ASID 0 is the shared/boot context (`L1`). Parallel arrays (not a
// struct-of-slot array) keep every PageTable 4 KiB-aligned and every UserRegion 16 KiB-aligned (a packed
// 28 KiB slot would misalign the backing on slots >= 1). ~224 KiB BSS, zeroed by `_start`.

/// Number of per-task user address-space slots (STOP tripwire: this cap is deliberate — do not raise it
/// to satisfy a demo; a real allocator/paged user memory is a later arc).
pub const USER_SLOTS: usize = 8;

static mut SLOT_L1: [PageTable; USER_SLOTS] = [const { PageTable([0; 512]) }; USER_SLOTS];
static mut SLOT_L2: [PageTable; USER_SLOTS] = [const { PageTable([0; 512]) }; USER_SLOTS];
static mut SLOT_L3: [PageTable; USER_SLOTS] = [const { PageTable([0; 512]) }; USER_SLOTS];
static mut SLOT_BACKING: [SlotBacking; USER_SLOTS] =
    [const { SlotBacking([0; USER_STATIC_SIZE]) }; USER_SLOTS];
/// Allocation state, one flag per slot. Atomic so `alloc`/`teardown` are race-free across cores.
static SLOT_USED: [AtomicBool; USER_SLOTS] = [const { AtomicBool::new(false) }; USER_SLOTS];

/// ELF-2 — the LIVE-TASK refcount for each slot (its shared address space). ELF-1 ran one task per slot,
/// so a slot's lifetime was "alloc … single task exits … teardown". ELF-2 lets several EL0 THREADS share
/// one slot's TTBR0/ASID (`SYS_THREAD_SPAWN`), so the slot's address space must outlive the FIRST thread to
/// exit and be torn down only when the LAST leaves. This counter is that guard: `alloc_user_slot` seeds it
/// to 1 (the initial owner task), `slot_thread_retain` bumps it per additional thread, and
/// `teardown_user_slot` decrements — doing the real ASID-flush + slot-free only on the 1->0 edge. Indexed by
/// slot (`asid - 1`). Untouched by the shared-window (ASID 0) / kernel (ttbr0 0) tasks, which never call
/// teardown.
static SLOT_REFCOUNT: [AtomicU32; USER_SLOTS] = [const { AtomicU32::new(0) }; USER_SLOTS];

/// STORM-HEADROOM — how many of the `USER_SLOTS` address-space slots are unclaimed right now. Reads
/// only (one relaxed-ordered flag per slot), safe from any core; never consulted on an allocation
/// path — `alloc_user_slot`'s CAS is the only thing that may decide a slot's fate, and a count taken
/// here is stale the instant it is returned.
///
/// It exists because the slot pool is the resource the `MAX_PROCS` block stakes its whole argument
/// on: 6 background rows are meant to leave 2 EL0 slots free for a foreground `run` and the launcher
/// fixtures. That reserve is a claim about a live system, and nothing on the wire ever stated it. The
/// `storm` verb samples it at its launch boundaries so a bench capture says whether the reserve
/// actually survived a full fleet, rather than whether it was intended to.
pub fn user_slots_free() -> usize {
    (0..USER_SLOTS).filter(|&s| !SLOT_USED[s].load(Ordering::Acquire)).count()
}

/// ELF-2 — register one more live EL0 thread against the slot owning `asid` (the shared address space a
/// `SYS_THREAD_SPAWN` adds a task to). Balanced by that thread's eventual `teardown_user_slot` call at exit.
/// MUST be called on a live slot (refcount already >= 1 from the initial owner) BEFORE the new thread can be
/// dispatched, so no thread exit can drive the count to 0 while another is still being wired up.
pub fn slot_thread_retain(asid: u64) {
    debug_assert!(asid >= 1 && asid as usize <= USER_SLOTS, "retain: asid out of range");
    let prev = SLOT_REFCOUNT[(asid - 1) as usize].fetch_add(1, Ordering::AcqRel);
    debug_assert!(prev >= 1, "slot_thread_retain on a slot with no live owner");
}

/// The boot/shared context TTBR0 value: `&L1 | (ASID 0 << 48)` == `&L1` (identity-mapped, so PA == VA).
/// Kernel tasks and the shared-window (M6b/M6e) EL0 tasks run under this root.
pub fn boot_ttbr0() -> u64 {
    &raw const L1 as u64
}

/// ASID assigned to slot `s` (1..=USER_SLOTS; ASID 0 is reserved for the boot/shared context).
#[inline]
fn slot_asid(s: usize) -> u64 {
    (s + 1) as u64
}

/// The TTBR0 value that installs slot `s`'s address space: `slot_l1_pa | (asid << 48)`.
pub fn slot_ttbr0(s: usize) -> u64 {
    debug_assert!(s < USER_SLOTS);
    let l1_pa = unsafe { (&raw const SLOT_L1).cast::<PageTable>().add(s) as u64 };
    l1_pa | (slot_asid(s) << 48)
}

/// Kernel-side identity pointer into slot `s`'s 16 KiB backing. The kernel copies the program and plants
/// data sentinels through THIS pointer — a Global, EL1-RW identity mapping reachable under any root —
/// never through the (ASID-tagged, EL0) user-window VA. A72 L1 caches are PIPT, so writes here are
/// coherent with the EL0 fetch/read of the same frame at the aliased user VA.
pub fn slot_backing_ptr(s: usize) -> *mut u8 {
    debug_assert!(s < USER_SLOTS);
    unsafe { (&raw mut SLOT_BACKING).cast::<SlotBacking>().add(s).cast::<u8>() }
}

/// Claim a free slot and build its private translation tables, returning the slot id. Pool-only (no heap),
/// so it is safe to call off the dispatch path. Returns `None` if all 8 slots are in use (STOP tripwire).
pub fn alloc_user_slot() -> Option<usize> {
    for s in 0..USER_SLOTS {
        if SLOT_USED[s]
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            // ELF-2: seed the live-task refcount to 1 for the initial owner. The store precedes
            // `build_slot`'s publishing barrier and any dispatch onto this slot, so a
            // `slot_thread_retain` (only reachable once a task runs on the slot) always sees >= 1.
            SLOT_REFCOUNT[s].store(1, Ordering::Release);
            unsafe { build_slot(s) };
            return Some(s);
        }
    }
    None
}

/// Claim `out.len()` slots at once, filling `out` with their ids; return whether ALL were obtained. On a
/// partial failure (the pool exhausts mid-request) this RELEASES the slots already claimed in this call and
/// returns `false`, so a multi-slot request never leaks earlier-claimed slots — the M6d review fold: the
/// four sequential `alloc_user_slot()?` calls in `m6d_setup` leaked earlier slots when a later one failed
/// (latent at 4/8, real once a demo allocates near the cap). A slot released here was never installed in any
/// core's `TTBR0` (a slot goes live only when a task is dispatched onto it), so no core cached a translation
/// under its ASID — clearing the used-flag is the whole unwind, NO `TLBI` needed (unlike `teardown_user_slot`,
/// which retires a slot whose ASID went live). STOP tripwire: a request larger than `USER_SLOTS` must FAIL
/// here, never grow the pool — raising the cap is a later arc (a real user-memory allocator).
pub fn alloc_user_slots(out: &mut [usize]) -> bool {
    let mut n = 0;
    while n < out.len() {
        match alloc_user_slot() {
            Some(s) => {
                out[n] = s;
                n += 1;
            }
            None => {
                for &s in &out[..n] {
                    SLOT_USED[s].store(false, Ordering::Release); // never installed -> no TLBI
                }
                return false;
            }
        }
    }
    true
}

/// Build slot `s`'s L1/L2/L3 by COPYING the finished boot tables (511/512 L1 entries, and every kernel
/// identity leaf, are byte-identical — so kernel code that runs while this root is live resolves its
/// .text/heap/stack/device mappings exactly) then patching the user branch: the slot's user-window L3
/// leaves point at the slot's OWN backing frames (nG, starting as RW data pages for the code copy), and
/// `L1[0] -> slot L2 -> slot L3`. The user pages have never been walked under this slot's ASID (fresh
/// tables; any prior tenant's ASID was flushed at its teardown), so a `dsb ishst` to publish the stores
/// to the Inner-Shareable walkers is all that is needed before a TTBR0 points here — no TLBI.
unsafe fn build_slot(s: usize) {
    // STOP TRIPWIRE (per-slot tables freeze the boot kernel mappings at COPY time): the loop below COPIES
    // the boot L1/L2/L3 into this slot, so the slot holds a FROZEN snapshot of every kernel mapping as it
    // stood when the slot was built. Any post-boot edit to a KERNEL mapping (a new device window, a kernel
    // W^X/permission flip, a heap remap) is INVISIBLE to already-built slots — it MUST be mirrored into all
    // live slot L1/L2/L3 copies (or force a slot rebuild + TLBI). Today this holds because M6b/M6d/M6f only
    // ever edit USER leaves, which are per-slot by construction; a kernel-mapping edit is out of that lane.
    let user_pa = &raw const USER_REGION as u64; // the shared user VA every slot re-maps to its backing
    let user_end = user_pa + USER_REGION_SIZE as u64;
    let l2_idx = (user_pa >> 21) as usize;
    let backing = slot_backing_ptr(s) as u64; // this slot's backing PA (identity-mapped)

    let boot_l1 = &raw const L1 as *const u64;
    let boot_l2 = &raw const L2_USER as *const u64;
    let boot_l3 = &raw const L3_USER as *const u64;
    let sl1 = unsafe { (&raw mut SLOT_L1).cast::<PageTable>().add(s).cast::<u64>() };
    let sl2 = unsafe { (&raw mut SLOT_L2).cast::<PageTable>().add(s).cast::<u64>() };
    let sl3 = unsafe { (&raw mut SLOT_L3).cast::<PageTable>().add(s).cast::<u64>() };
    unsafe {
        // WC-B (F1): SCRUB this slot's FB REGION before the new tenant can reach it. A slot is RECYCLED —
        // `teardown_user_slot` retires the ASID and its mappings but never touches the backing bytes, and
        // the ELF/blob load only writes the 16 KiB PROGRAM window, so the FB region carries the PREVIOUS
        // tenant's frame. Under ELF-3 that was one stale 4 KiB surface; WC-B makes it 512 KiB across 8
        // window slots AND directly verb-reachable (`SYS_WIN_CREATE` maps up to 16 of those pages EL0-RW),
        // so the leak is closed here rather than left as a documented class.
        //
        // BUILD is the right point, not map: it is exactly the slot-recycle boundary the leak lives on, it
        // runs once per tenant instead of once per map, and — unlike zeroing inside `map_slot_fb_win` — it
        // cannot wipe a caller's OWN pixels, which zeroing on map would do to any second `SYS_FB_MAP`
        // (documented idempotent) or re-create. The program window is deliberately NOT scrubbed here: the
        // loader owns it and already writes/zeroes what it needs, and that path is unchanged.
        core::ptr::write_bytes(
            slot_backing_ptr(s).add(USER_REGION_SIZE),
            0,
            USER_STATIC_SIZE - USER_REGION_SIZE,
        );
        for i in 0..512 {
            sl1.add(i).write_volatile(boot_l1.add(i).read_volatile());
            sl2.add(i).write_volatile(boot_l2.add(i).read_volatile());
            sl3.add(i).write_volatile(boot_l3.add(i).read_volatile());
        }
        // Point the slot's user-window L3 leaves at the slot's own backing (nG data pages — RW so the
        // caller can copy the program in; `protect_user_slot_code` flips page 0 to EL0-RX/EL1-RO).
        let mut va = user_pa;
        while va < user_end {
            let pa = backing + (va - user_pa);
            sl3.add(((va >> 12) & 0x1FF) as usize).write_volatile(user_data_page(pa));
            va += 0x1000;
        }
        // Redirect the table branch into the slot's own L2/L3.
        sl2.add(l2_idx).write_volatile(table_desc(unsafe {
            (&raw const SLOT_L3).cast::<PageTable>().add(s) as u64
        }));
        sl1.add(0).write_volatile(table_desc(unsafe {
            (&raw const SLOT_L2).cast::<PageTable>().add(s) as u64
        }));
        // Publish the descriptors to every Inner-Shareable table walker before any core's TTBR0 walks
        // them (a slot built on the BSP is first used by a task on an AP). No TLBI: fresh ASID, no stale.
        core::arch::asm!("dsb ishst", "isb", options(nostack, preserves_flags));
    }
}

/// Flip slot `s`'s CODE page(s) `[user_va, user_va+len)` from RW data pages to their final EL0-RX/EL1-RO
/// shape (`user_code_page`), mirroring `protect_user_code`'s maintenance (descriptor write -> `dsb ishst`
/// -> per-page broadcast `tlbi vaae1is` (all-ASID; operand `va >> 12`) -> `dsb ish` -> `isb`). No AT probe
/// (unlike the shared-window `protect_user_code`): the slot's mapping is not live under the BSP's current
/// TTBR0, so an AT here would translate the shared window, not this slot. The flip precedes the slot's
/// task ever running, so no concurrent walk under this ASID -> the permission-only leaf rewrite is
/// break-before-make-exempt.
pub unsafe fn protect_user_slot_code(s: usize, len: usize) {
    unsafe { protect_user_slot_code_range(s, 0, len) };
}

/// ELF-1 generalisation of `protect_user_slot_code`: flip the pages covering `[user_va+off, user_va+off+len)`
/// of slot `s` from RW data pages to their final EL0-RX/EL1-RO code shape. `protect_user_slot_code` is the
/// `off == 0` case (the flat-binary path + the U4/K2 fixtures — one code page at the window base). The ELF
/// loader calls this once per PT_LOAD segment carrying PF_X, so an executable segment at ANY page offset
/// within the window becomes code while the R/W (data) segments stay `user_data_page` (RW, UXN). Pages are
/// flipped whole (the covered `[off, off+len)` range is rounded out to page boundaries by the loop, which
/// walks from the page containing `off`); the loader lays code and data segments on DISTINCT pages (its
/// linker forces a page gap), so a flip never straddles a data segment. Same maintenance as the base case
/// (descriptor write -> `dsb ishst` -> per-page broadcast `tlbi vaae1is` -> `dsb ish` -> `isb`), and the
/// same break-before-make exemption (the slot's task does not yet run, so no concurrent walk under its ASID).
pub unsafe fn protect_user_slot_code_range(s: usize, off: usize, len: usize) {
    debug_assert!(s < USER_SLOTS);
    debug_assert!(off + len <= USER_REGION_SIZE, "protect range outside the slot window");
    let user_pa = &raw const USER_REGION as u64;
    let backing = slot_backing_ptr(s) as u64;
    let sl3 = unsafe { (&raw mut SLOT_L3).cast::<PageTable>().add(s).cast::<u64>() };
    let start = user_pa + (off as u64 & !0xFFF); // first page of the range
    let end = user_pa + off as u64 + len as u64; // exclusive
    unsafe {
        let mut va = start;
        while va < end {
            let pa = backing + (va - user_pa);
            sl3.add(((va >> 12) & 0x1FF) as usize).write_volatile(user_code_page(pa));
            va += 0x1000;
        }
        core::arch::asm!("dsb ishst", options(nostack, preserves_flags));
        let mut va = start;
        while va < end {
            core::arch::asm!("tlbi vaae1is, {}", in(reg) (va >> 12), options(nostack, preserves_flags));
            va += 0x1000;
        }
        core::arch::asm!("dsb ish", "isb", options(nostack, preserves_flags));
    }
}

/// ELF-3: the EL0 VA of slot `s`'s FB info page (read-only geometry) — the shared user-window VA hole
/// immediately above the 16 KiB program window. Same VA in every slot; the FRAME differs per slot (its
/// own backing), installed by `map_slot_fb`.
pub fn fb_info_va() -> u64 {
    (&raw const USER_REGION as u64) + USER_REGION_SIZE as u64
}
/// ELF-3: the EL0 VA of slot `s`'s FB surface (EL0-RW off-screen draw target), one page above the info
/// page. This is what SYS_FB_MAP returns to EL0. WC-B: identical to `fb_win_surface_va(0)` — the compat
/// surface IS window slot 0, at the same VA it always had.
pub fn fb_surface_va() -> u64 {
    fb_info_va() + FB_INFO_SIZE as u64
}

/// WC-B: the EL0 VA of window surface slot `w` (0..FB_WIN_SLOTS) in the caller's address space. Same VA
/// in every process slot; the FRAME differs per process (its own backing), installed by `map_slot_fb_win`.
pub fn fb_win_surface_va(w: usize) -> u64 {
    debug_assert!(w < FB_WIN_SLOTS);
    fb_info_va() + FB_INFO_SIZE as u64 + (w * FB_WIN_SLOT_SIZE) as u64
}

/// WC-B: kernel-side identity pointer to process-slot `s`'s window surface slot `w` (EL1-RW; the kernel
/// reads it to composite / checksum, EL0 draws through the aliased EL0-RW VA — A72 PIPT caches keep the
/// two coherent, exactly as for the single ELF-3 surface).
pub fn slot_fb_win_surface_ptr(s: usize, w: usize) -> *mut u8 {
    debug_assert!(s < USER_SLOTS && w < FB_WIN_SLOTS);
    unsafe { slot_backing_ptr(s).add(USER_REGION_SIZE + FB_INFO_SIZE + w * FB_WIN_SLOT_SIZE) }
}
/// ELF-3: kernel-side identity pointer to slot `s`'s FB info page (EL1-RW, the kernel writes geometry
/// here; the EL0 alias is read-only).
pub fn slot_fb_info_ptr(s: usize) -> *mut u8 {
    debug_assert!(s < USER_SLOTS);
    unsafe { slot_backing_ptr(s).add(USER_REGION_SIZE) }
}
/// ELF-3: kernel-side identity pointer to slot `s`'s FB surface (EL1-RW; the kernel reads it to composite
/// / checksum, EL0 draws into the aliased EL0-RW VA — A72 PIPT caches keep the two coherent).
pub fn slot_fb_surface_ptr(s: usize) -> *mut u8 {
    debug_assert!(s < USER_SLOTS);
    unsafe { slot_backing_ptr(s).add(USER_REGION_SIZE + FB_INFO_SIZE) }
}

/// ELF-3: map slot `s`'s FB info + surface pages into its EL0 window (from SYS_FB_MAP). The two leaves
/// were copies of the boot L3 (identity RAM, EL1-only — the reserved hole); this repoints them at the
/// slot's OWN backing frames: the info page EL0-RO (`user_ro_page`), the surface EL0-RW Normal-cacheable
/// (`user_data_page`). Because the output address changes on a live valid leaf (not a permission-only
/// change), the sequence is proper BREAK-BEFORE-MAKE: invalidate the old leaf, broadcast-TLBI, THEN write
/// the new leaf and publish. Safe against a sibling-core drawing thread because SYS_FB_MAP is called by the
/// process BEFORE it spawns any drawing thread, so no core walks these VAs during the flip; the `dsb ish`
/// after the TLBI + `dsb ishst` after the new store make the new mapping visible to every Inner-Shareable
/// walker before the first draw. Editing only USER leaves in the slot's PRIVATE L3 (in-lane; the per-slot
/// freeze note in `build_slot` is about KERNEL mappings).
pub unsafe fn map_slot_fb(s: usize) {
    debug_assert!(s < USER_SLOTS);
    unsafe { map_slot_fb_info(s) };
    // WC-B: the ELF-3 compat surface is window slot 0's FIRST page — byte-identical mapping to before.
    unsafe { map_slot_fb_win(s, 0, FB_SURFACE_SIZE / 0x1000) };
}

/// WC-B: map slot `s`'s RO info page alone (the half of `map_slot_fb` every window path shares). Same
/// break-before-make sequence and the same `user_ro_page` shape the combined ELF-3 call used.
///
/// WC-B (F3): IDEMPOTENT — if the leaf is ALREADY the wanted descriptor this returns without touching it.
/// That is not an optimisation, it is a correctness requirement now that `SYS_WIN_CREATE` reaches this
/// path: ELF-3's break-before-make was safe only because `SYS_FB_MAP` ran before the process spawned any
/// drawing thread, and a window create carries no such ordering — a sibling thread reading the info page
/// during the BREAK window would take a spurious, FATAL data abort on a perfectly correct program. Since
/// the descriptor is a pure function of the slot, a no-op re-map is the whole fix; a leaf that differs
/// (the first map, from the reserved identity descriptor) has by definition never been a valid EL0 info
/// page for this tenant, so the BBM below is unreachable by a legitimate concurrent reader.
pub unsafe fn map_slot_fb_info(s: usize) {
    debug_assert!(s < USER_SLOTS);
    let info_va = fb_info_va();
    let info_pa = slot_fb_info_ptr(s) as u64;
    let sl3 = unsafe { (&raw mut SLOT_L3).cast::<PageTable>().add(s).cast::<u64>() };
    let info_idx = ((info_va >> 12) & 0x1FF) as usize;
    if unsafe { sl3.add(info_idx).read_volatile() } == user_ro_page(info_pa) {
        return; // already mapped for this slot — no leaf edit, so no break window
    }
    unsafe {
        sl3.add(info_idx).write_volatile(0); // break
        core::arch::asm!("dsb ishst", options(nostack, preserves_flags));
        core::arch::asm!("tlbi vaae1is, {}", in(reg) (info_va >> 12), options(nostack, preserves_flags));
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));
        sl3.add(info_idx).write_volatile(user_ro_page(info_pa)); // make
        core::arch::asm!("dsb ishst", "isb", options(nostack, preserves_flags));
    }
}

/// WC-B: map the first `pages` pages of process-slot `s`'s window surface slot `w` into its EL0 window,
/// EL0-RW Normal-cacheable (`user_data_page`) — the SAME leaf shape, and therefore the same MMU
/// attributes, the single ELF-3 surface page has always had. `pages` is the NEGOTIATED page-multiple size
/// (`w*h*4` rounded up), never the whole 16-page slot: the remainder keeps its reserved EL1-only identity
/// leaf, so a process that asked for 32×32 cannot reach the rest of its own slot, let alone another's.
///
/// Same proper BREAK-BEFORE-MAKE as `map_slot_fb` (the output address changes on a live valid leaf):
/// invalidate, broadcast-TLBI, then write the new leaf and publish.
pub unsafe fn map_slot_fb_win(s: usize, w: usize, pages: usize) {
    debug_assert!(s < USER_SLOTS && w < FB_WIN_SLOTS);
    debug_assert!(pages >= 1 && pages <= FB_WIN_SLOT_SIZE / 0x1000);
    let base_va = fb_win_surface_va(w);
    let base_pa = slot_fb_win_surface_ptr(s, w) as u64;
    let sl3 = unsafe { (&raw mut SLOT_L3).cast::<PageTable>().add(s).cast::<u64>() };
    unsafe {
        // Break: invalidate the old (reserved identity, EL1-only) leaves for the whole negotiated range.
        for p in 0..pages {
            let va = base_va + (p * 0x1000) as u64;
            sl3.add(((va >> 12) & 0x1FF) as usize).write_volatile(0);
        }
        core::arch::asm!("dsb ishst", options(nostack, preserves_flags));
        for p in 0..pages {
            let va = base_va + (p * 0x1000) as u64;
            core::arch::asm!("tlbi vaae1is, {}", in(reg) (va >> 12), options(nostack, preserves_flags));
        }
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));
        // Make: install this window's surface frames EL0-RW, then publish + sync this core.
        for p in 0..pages {
            let va = base_va + (p * 0x1000) as u64;
            let pa = base_pa + (p * 0x1000) as u64;
            sl3.add(((va >> 12) & 0x1FF) as usize).write_volatile(user_data_page(pa));
        }
        core::arch::asm!("dsb ishst", "isb", options(nostack, preserves_flags));
    }
}

/// WC-B: the `map_slot_fb_win` inverse, for `SYS_WIN_CLOSE` and ASID teardown. Restores the leaves to
/// their RESERVED state — the boot L3's own descriptors for these VAs (identity RAM, EL1-only), exactly
/// what `build_slot` copied in — so a closed window's surface is unreachable from EL0 the instant the
/// TLBI completes, and a later re-create goes through the same break-before-make path as the first.
///
/// Unlike the map path this may run while the owner still has threads live (a close is a syscall a
/// drawing sibling cannot be ordered against), so the invalidate-then-broadcast-TLBI-then-`dsb ish`
/// ORDER is load-bearing: any concurrent EL0 access to a closed surface must fault, never read a stale
/// mapping. That fault is the intended fail-closed outcome, not a regression.
pub unsafe fn unmap_slot_fb_win(s: usize, w: usize, pages: usize) {
    debug_assert!(s < USER_SLOTS && w < FB_WIN_SLOTS);
    debug_assert!(pages >= 1 && pages <= FB_WIN_SLOT_SIZE / 0x1000);
    let base_va = fb_win_surface_va(w);
    let sl3 = unsafe { (&raw mut SLOT_L3).cast::<PageTable>().add(s).cast::<u64>() };
    let boot_l3 = &raw const L3_USER as *const u64;
    unsafe {
        for p in 0..pages {
            let va = base_va + (p * 0x1000) as u64;
            sl3.add(((va >> 12) & 0x1FF) as usize).write_volatile(0);
        }
        core::arch::asm!("dsb ishst", options(nostack, preserves_flags));
        for p in 0..pages {
            let va = base_va + (p * 0x1000) as u64;
            core::arch::asm!("tlbi vaae1is, {}", in(reg) (va >> 12), options(nostack, preserves_flags));
        }
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));
        for p in 0..pages {
            let idx = (((base_va + (p * 0x1000) as u64) >> 12) & 0x1FF) as usize;
            sl3.add(idx).write_volatile(boot_l3.add(idx).read_volatile());
        }
        core::arch::asm!("dsb ishst", "isb", options(nostack, preserves_flags));
    }
}

/// ELF-1 witness support: read slot `s`'s L3 leaf descriptor for the user window page containing `va`
/// (a window VA in `[user_region, user_region+size)`). Lets the loader witness assert, kernel-side, that
/// an executable segment's page landed as a CODE leaf (RO-both + EL0-executable) and a data segment's page
/// as a DATA leaf (EL0+EL1 RW, never executable) — the per-segment permission proof.
pub fn slot_leaf_desc(s: usize, va: u64) -> u64 {
    debug_assert!(s < USER_SLOTS);
    let sl3 = unsafe { (&raw const SLOT_L3).cast::<PageTable>().add(s).cast::<u64>() };
    unsafe { sl3.add(((va >> 12) & 0x1FF) as usize).read_volatile() }
}

/// True iff slot `s`'s page for window VA `va` is a CODE leaf: read-only at both ELs (AP=0b11) AND
/// EL0-executable (UXN clear). The exact shape `user_code_page` writes.
pub fn slot_page_is_code(s: usize, va: u64) -> bool {
    let d = slot_leaf_desc(s, va);
    (d & (0b11 << 6)) == AP_RO_ALL && (d & DESC_UXN) == 0
}

/// True iff slot `s`'s page for window VA `va` is a DATA leaf: EL0+EL1 read-write (AP=0b01) AND
/// unprivileged-execute-never (UXN set). The shape `user_data_page` writes.
pub fn slot_page_is_data(s: usize, va: u64) -> bool {
    let d = slot_leaf_desc(s, va);
    (d & (0b11 << 6)) == AP_EL0 && (d & DESC_UXN) != 0
}

/// Release ONE live task's hold on the slot owning `asid` (1..=USER_SLOTS) at task exit. Called for EVERY
/// slot-bound EL0 task's exit (`sched::exit`); the ELF-1 single-task case and the ELF-2 multi-thread case
/// share this path.
///
/// TWO-PHASE, order load-bearing (the exact class of bug QEMU cannot see):
///  1. ALWAYS repoint THIS core's TTBR0 off the slot root to the boot root (`&L1 | ASID 0`) + `isb`. This
///     runs on every thread's exit, not only the last — so no core is ever left with a torn-down (or, for a
///     not-yet-last thread, a soon-to-be-torn-down) slot root live in TTBR0. That is what makes the
///     multi-core shared-ASID case sound: `build_slot`'s "the ASID was flushed at teardown and no core can
///     speculatively re-cache it" invariant (relied on at the slot's next `alloc`) holds because each thread
///     repoints its own core away from the slot root as it exits, so at the final flush no OTHER core holds
///     the root live to speculatively refill under it.
///  2. Decrement the live-task refcount. On a NON-final release (`prev != 1`) stop here: the slot's address
///     space is still in use by sibling threads — no ASID flush, no free. Only the FINAL release (1->0 edge)
///     broadcast-invalidates the ASID (`dsb ishst; tlbi aside1is,(asid<<48); dsb ish; isb` — broadcast
///     because a reused slot may next run on ANOTHER core), clears the handle row, and frees the slot.
///
/// ELF-1 behavior is a special case: one task, refcount 1, so the first (and only) release is the final one
/// and the flush+free run exactly as before.
pub unsafe fn teardown_user_slot(asid: u64) {
    debug_assert!(asid >= 1 && asid as usize <= USER_SLOTS, "teardown: asid out of range");
    // The ASIDE1IS operand carries the ASID in Xt[63:48]; assert `asid << 48` round-trips (a mis-encoded
    // operand would flush the wrong ASID — silent on QEMU, a stale-entry bug on metal).
    debug_assert_eq!((asid << 48) >> 48, asid, "teardown: ASID does not fit Xt[63:48]");
    let boot = &raw const L1 as u64; // boot root, ASID 0
    // Phase 1 — repoint THIS core off the slot root unconditionally (see the two-phase note above).
    unsafe {
        core::arch::asm!(
            "msr TTBR0_EL1, {boot}",
            "isb",
            boot = in(reg) boot,
            options(nostack, preserves_flags),
        );
    }
    // Phase 2 — only the LAST live task flushes the ASID + frees the slot. A non-final release leaves the
    // shared address space intact for the surviving sibling threads (this core is already off it, above).
    if SLOT_REFCOUNT[(asid - 1) as usize].fetch_sub(1, Ordering::AcqRel) != 1 {
        return;
    }
    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "tlbi aside1is, {asidreg}",
            "dsb ish",
            "isb",
            asidreg = in(reg) (asid << 48),
            options(nostack, preserves_flags),
        );
    }
    // U5 teardown-clear (folds U4's one deferred lifecycle note): wipe this ASID's per-process handle row so a
    // future slot-reuse always starts from an empty capability table. Ordering is load-bearing — clear the row
    // BEFORE releasing the used-flag below, NOT after: once `SLOT_USED` goes false another core's
    // `alloc_user_slot` may claim this same slot (same ASID) and begin populating its row, so a clear placed
    // after the release could wipe the NEW owner's capabilities. The clear lives in `syscall` (which owns the
    // table); both modules are `#[cfg(feature = "baremetal")]`, so the call is always available here. The dead
    // ASID is already off this core's live TTBR0 (repointed above), and no task runs under it (the owner is
    // exiting), so nothing observes a torn intermediate state.
    super::syscall::clear_handle_row(asid);
    // VUG-BG teardown-clear: same funnel, same ordering rationale, same reason. The DETACHED bit is
    // per-ASID state, and ASIDs are RECYCLED — so a slot last used by a `bg` spawn would otherwise hand
    // its stale bit to whatever claims the slot next, and that next tenant would read "I was launched
    // detached" and (for VUG.ELF) never stop. `run_user_image` and `spawn_user_image_bg` both write the
    // bit explicitly, which covers the two paths that can launch a FAT-loaded image; this clear is what
    // covers every OTHER producer of an EL0 address space — `sys_spawn`'s `load_program_into_slot` and
    // the dozen in-kernel `alloc_user_slot` fixture launchers — without asking each of them to remember.
    // One durable line at the funnel beats a rule every future launcher has to be told about.
    super::syscall::clear_detached(asid);
    // VUGMIN teardown-clear: the same funnel and the same ordering, for the HIDDEN bit. It matters MORE
    // here than for DETACHED, not less: a stale detached bit gives the next tenant an uncapped frame
    // budget, whereas a stale hidden bit gives it a vug that comes up already idling — a window that
    // never draws — having never been hidden at all. Both bits are per-ASID and ASIDs are recycled, so
    // both are cleared here, before the slot is released for reuse below.
    super::syscall::clear_hidden(asid);
    SLOT_USED[(asid - 1) as usize].store(false, Ordering::Release);
}

/// Deterministic, on-metal detector for the nG discipline (the arc's #1 metal risk) — the M6d analogue
/// of M6b's `tlb_warm`. IRQ-masked on the calling core, it swaps TTBR0 between slot `a`'s and slot `b`'s
/// roots and does REAL EL1 loads of the SAME user VA (`user_region + off`, an AP=0b01 data page readable
/// at EL1 on this PAN-less A72). Real loads consult the TLB, so if the user leaf were Global (nG bug) the
/// first load caches a global entry for the VA and the second (different ASID) HITS it -> returns slot
/// `a`'s frame under both -> `false`. With correct nG the second load misses the ASID-`a` entry, re-walks
/// slot `b`, and returns slot `b`'s frame. QEMU models no TLB (re-walks every access) so it always sees
/// the right frame -> `true`. Returns whether both reads matched their planted sentinels (and the two
/// differ). Cleans up its cached entries with an all-ASID broadcast TLBI of the probe VA.
pub unsafe fn probe_slot_isolation(a: usize, b: usize, off: u64, expect_a: u64, expect_b: u64) -> bool {
    debug_assert!(a < USER_SLOTS && b < USER_SLOTS);
    // The no-TLBI TTBR0 swap between the two roots is architecturally legal ONLY because they carry
    // DISTINCT ASIDs: that is what lets the second (ASID-b) load miss the ASID-a entry the first load
    // cached and re-walk to slot b. Two equal slots would share an ASID and the probe would read slot a's
    // frame under both roots — silently "passing" the isolation check it exists to make fail on a bug.
    debug_assert!(a != b, "probe_slot_isolation requires distinct slots (distinct ASIDs)");
    let va = (&raw const USER_REGION as u64) + off;
    let root_a = slot_ttbr0(a);
    let root_b = slot_ttbr0(b);
    let (r_a, r_b): (u64, u64);
    unsafe {
        core::arch::asm!(
            "mrs {daif}, DAIF",
            "msr DAIFSet, #2",          // mask IRQ: no preempt may reswap TTBR0 mid-probe
            "mrs {saved}, TTBR0_EL1",
            "msr TTBR0_EL1, {ra}",
            "isb",
            "ldr {ra_out}, [{va}]",     // caches (VA -> slot a frame) under ASID a
            "msr TTBR0_EL1, {rb}",
            "isb",
            "ldr {rb_out}, [{va}]",     // correct nG: misses ASID a, re-walks slot b; global nG-bug: hits a
            "msr TTBR0_EL1, {saved}",
            "isb",
            "msr DAIF, {daif}",
            va = in(reg) va,
            ra = in(reg) root_a,
            rb = in(reg) root_b,
            ra_out = out(reg) r_a,
            rb_out = out(reg) r_b,
            saved = out(reg) _,
            daif = out(reg) _,
            options(nostack, preserves_flags),
        );
        // Hygiene: drop the probe's cached entries for this VA across the domain (they are nG/ASID-tagged
        // and this core never revisits the VA, but keep the TLB clean of probe artifacts).
        core::arch::asm!(
            "dsb ishst",
            "tlbi vaae1is, {}",
            "dsb ish",
            "isb",
            in(reg) (va >> 12),
            options(nostack, preserves_flags),
        );
    }
    r_a == expect_a && r_b == expect_b && expect_a != expect_b
}

// --- SMP-8: CPUECTLR_EL1.SMPEN (Cortex-A72 IMPDEF bit 6) — required config, plus its witness. ---
//
// The A72 TRM makes SMPEN required configuration, not an option: it must be set BEFORE the caches or
// the MMU are enabled on a core, and while it is clear that core does not participate in coherency —
// its cache lines are not snooped and, crucially, its exclusive monitor does not work across the
// cluster, so the FIRST `ldaxr/stlxr` pair it executes can spin forever. That is exactly the shape of
// the P53 metal report ("only procs 2 & 4 running regularly"): an AP that arrives and never reports.
// We have never set it and never checked it, so if the GPU firmware's armstub leaves it clear on some
// APs, those APs are non-coherent from the moment they reach us. Linux's boot/PSCI stubs set this bit
// for the same reason; setting it is what the hardware documentation requires of any code that owns
// the core, so the SET below is ALWAYS ON — it protects nothing and weakens nothing.
//
// Where: at EL2, in `drop_to_el1`, before the `eret`. That placement is deliberate on three counts.
//   * CPUECTLR_EL1 is IMPLEMENTATION DEFINED; an EL1 read can be trapped to EL2 by ACTLR_EL2, which
//     we never program (it resets UNKNOWN). Taking an unexpected exception on a secondary is the very
//     failure under investigation, so the access must happen at EL2, where it cannot trap.
//   * `drop_to_el1` is on BOTH paths — the BSP reaches it from `__rust_boot`, every AP from
//     `__secondary_rust` — so one insertion covers all four cores with identical code.
//   * It runs with the MMU and caches still off, which is where the TRM wants the bit set.
//
// The raw PRE-fix value is recorded per core in `SMP8_CPUECTLR` so `[smp8]` can report whether
// firmware had already set SMPEN. These stores happen MMU-off (DRAM-direct) while every later read is
// Normal-cacheable — the mismatched-attributes hazard of §CORE3-SMP. Two things make the read sound:
// the object is exactly one 64-byte line, aligned and dedicated (nothing else ever shares it, so
// invalidating it can discard nothing), and the reader (`smp::report_smp8`) clean+invalidates that
// line to the PoC before loading it. A per-core magic word distinguishes "recorded 0" from "never
// recorded" — a genuinely all-zero CPUECTLR (SMPEN clear, everything else clear) is the single most
// interesting reading, so it must not be indistinguishable from an untouched BSS slot.
/// SMP-8 scratch: `[0..4]` = the raw CPUECTLR_EL1 each core read at EL2 before any fix, indexed by
/// MPIDR Aff0; `[4..8]` = `SMP8_MAGIC` once that core has written its slot. Exactly one cache line,
/// dedicated: `smp::report_smp8` invalidates it wholesale before reading.
#[repr(C, align(64))]
pub struct Smp8Slots(pub [u64; 8]);

/// Written by every core into `SMP8_CPUECTLR[4 + core]` to mark its reading as real.
pub const SMP8_MAGIC: u64 = 0x534d_5038; // "SMP8"

#[unsafe(no_mangle)]
pub static mut SMP8_CPUECTLR: Smp8Slots = Smp8Slots([0; 8]);

// --- EL2 -> EL1 drop. Every core is handed off at EL2; a normal OS runs at EL1, so we drop each core
// before it enables its MMU. MUST be naked asm: an ordinary Rust fn's prologue/epilogue would spill/
// reload x30 and adjust SP around the `eret`, and the eret skips the epilogue — corrupting the frame.
// Runs at EL2, MMU OFF, no stack traffic, x30 untouched; `eret`s back to the caller now at EL1 (same
// SP/frame — the standard "return to x30" drop trick). ---
// The SMP-8 probe body, spliced into `drop_to_el1` below.
//
// Why `#[cfg(feature = "pi")]`: this `drop_to_el1` is the **Pi/A72** drop — its only callers are
// `__rust_boot` (BSP) and `__secondary_rust` (APs), both bare-metal Pi. The other platforms have
// their own: `virt` uses `boot_virt::drop_to_el1` and the Jetson uses `boot_tegra::drop_to_el1`.
// But `mod boot` itself is NOT feature-gated (`arch/aarch64/mod.rs` declares it unconditionally),
// so this asm is *assembled into* `virt` and `tegra` images even though nothing there calls it —
// and `CPUECTLR_EL1` is IMPLEMENTATION DEFINED with a per-core encoding (on the A78AE it is
// `S3_0_C15_C1_4`, not the A72's `S3_1_C15_C2_1`). The cfg keeps an A72-specific system-register
// encoding out of every non-A72 image; it is about what gets assembled, not about who calls it.
//
// Register discipline: x1-x5 only. `drop_to_el1` is called as an ordinary no-argument `extern "C"`
// fn, so x1-x5 are dead on entry; x0 (the stub's own scratch) and x30 (the eret target) are untouched.
#[cfg(feature = "pi")]
macro_rules! smp8_probe {
    () => {
        r#"
    // --- SMP-8: record + set CPUECTLR_EL1.SMPEN at EL2, MMU/caches off (see the comment above). ---
    mrs   x1, mpidr_el1
    and   x1, x1, #0xff              // x1 = Aff0 (this core's slot index)
    mrs   x2, s3_1_c15_c2_1          // x2 = CPUECTLR_EL1 (A72 IMPDEF) — at EL2 this cannot trap
    // Bound the slot index BEFORE it indexes anything. The mask above admits Aff0 up to 255, and the
    // scaled store would then write up to 2040 bytes past the 64-byte record into adjacent BSS. The
    // Pi 4 is one cluster of 4, so this can't fire — but a store whose bound rests on a platform fact
    // rather than on a check is exactly the kind of latent corruption this arc exists to remove. An
    // out-of-range core skips the RECORD only; it still gets SMPEN set, which is the part that must
    // never be conditional.
    cmp   x1, #4
    b.hs  .Lsmp8_set
    adrp  x3, SMP8_CPUECTLR
    add   x3, x3, #:lo12:SMP8_CPUECTLR
    str   x2, [x3, x1, lsl #3]       // raw PRE-fix value -> slots[core]
    add   x4, x3, #32
    mov   x5, #0x5038
    movk  x5, #0x534d, lsl #16       // "SMP8"
    str   x5, [x4, x1, lsl #3]       // validity magic -> slots[4+core] (0 is a legal reading)
    dsb   sy                         // both stores complete to DRAM before we touch the register
.Lsmp8_set:
    tst   x2, #(1 << 6)              // SMPEN already set by firmware?
    b.ne  .Lsmp8_done
    orr   x2, x2, #(1 << 6)
    msr   s3_1_c15_c2_1, x2          // required config: coherency + a working cluster monitor
    isb
.Lsmp8_done:
"#
    };
}
#[cfg(not(feature = "pi"))]
macro_rules! smp8_probe {
    () => {
        ""
    };
}

core::arch::global_asm!(
    r#"
    .globl drop_to_el1
drop_to_el1:
"#,
    smp8_probe!(),
    r#"
    // MPIDR_EL1/MIDR_EL1 read at EL1 return VMPIDR_EL2/VPIDR_EL2 — seed them with the real values so
    // the SMP core-id read (smp.rs) stays correct at EL1.
    mrs   x0, mpidr_el1
    msr   vmpidr_el2, x0
    mrs   x0, midr_el1
    msr   vpidr_el2, x0
    // CPTR_EL2 = 0x33FF: clear TFP (bit 10) so EL1/EL0 FP/SIMD does NOT trap to EL2 (the kernel is
    // +neon; the GUI blits NEON, memcpy/fmt autovectorize). CPTR_EL2.TFP takes precedence over
    // CPACR_EL1.FPEN and resets UNKNOWN, so this must be explicit; 0x33FF keeps the non-VHE RES1 bits
    // set (do NOT 'msr cptr_el2, xzr').
    mov   x0, #0x33ff
    msr   cptr_el2, x0
    // MDCR_EL2 = 0: don't route EL1 debug/PMU exceptions to the (now abandoned) EL2 vectors.
    msr   mdcr_el2, xzr
    // CNTHCTL_EL2 EL1PCTEN+EL1PCEN=1: let EL1 read CNTPCT / use CNTP_* without trapping to EL2
    // (timer.rs touches these every tick). CNTVOFF_EL2=0 so CNTVCT shares the physical timebase.
    mrs   x0, cnthctl_el2
    orr   x0, x0, #0x3
    msr   cnthctl_el2, x0
    msr   cntvoff_el2, xzr
    // CPACR_EL1.FPEN=0b11 (bits [21:20]): the EL1-side FP/SIMD enable.
    mov   x0, #(0b11 << 20)
    msr   cpacr_el1, x0
    // HCR_EL2 = RW only (bit 31): EL1 executes AArch64; IMO/FMO/AMO cleared so a physical IRQ taken at
    // EL1 targets EL1 natively (no EL2 routing). Bare write (HCR_EL2 resets 0 on A72), not an RMW.
    mov   x0, #(1 << 31)
    msr   hcr_el2, x0
    // Land at EL1h (SPx = SP_EL1) with DAIF masked; SP_EL1 = current SP so the stack is continuous;
    // ELR_EL2 = the return address (x30), so the eret returns to our caller now running at EL1.
    mov   x0, sp
    msr   sp_el1, x0
    mov   x0, #0x3c5
    msr   spsr_el2, x0
    msr   elr_el2, x30
    isb
    eret
"#
);

unsafe extern "C" {
    /// Drop this core EL2 -> EL1 and return to the caller now executing at EL1. Call at EL2 with the
    /// MMU OFF, before `enable_mmu`. See the naked asm above for the full sequence and rationale.
    pub fn drop_to_el1();
}

// --- BootInfo for the bare-metal path (no UEFI to provide one). ---

/// A usable RAM region for the kernel heap. Placed at 32 MiB, 64 MiB long (≥ the 48 MiB HEAP_SIZE),
/// which clears the kernel image + stack (at 0x80000, a few MiB) and the firmware/DTB structures in
/// low memory. The Pi 4 has ≥ 1 GiB, so this is always backed.
static mut MEM_REGIONS: [MemoryRegion; 1] = [MemoryRegion {
    phys_start: 0x0200_0000,
    page_count: 0x4000, // 64 MiB / 4 KiB
    kind: MemoryRegionKind::Usable,
}];

static mut BOOT_INFO: BootInfo = BootInfo {
    framebuffer_addr: 0,
    framebuffer_size: 0,
    framebuffer_info: FrameBufferInfo {
        width: 0,
        height: 0,
        stride: 0,
        bytes_per_pixel: 4,
        pixel_format: PixelFormat::Unknown,
    },
    physical_memory_offset: 0,
    dtb_addr: 0,
    dtb_size: 0,
    rsdp_addr: 0,
    memory_regions_addr: 0,
    memory_regions_len: 0,
    edid_native_width: 0,
    edid_native_height: 0,
    edid_source: 0,
    mode_action: 0,
    // EDID-CARRY: the Pi 4 bare-metal path never runs the UEFI bootloader, so there is no EDID
    // protocol to read — the VideoCore mailbox hands over a framebuffer, not a panel descriptor.
    // `edid_block_valid: false` is the absent sentinel: `video::init_edid` prints `present=0` and
    // publishes nothing, rather than letting 128 zero bytes pass for a panel.
    edid_block: [0; 128],
    edid_block_valid: false,
    edid_total_len: 0,
    // INSTALL-SELF: aarch64 does not boot through the UEFI bootloader that reads its own ESP's FAT
    // volume serial, so the boot volume is unidentified here. 0 is the absent sentinel; the
    // installer's boot-device guard disarms on it (with a witness line) rather than guessing.
    boot_volume_serial: 0,
};

/// Synthesize the BootInfo the kernel expects. `dtb_addr` carries the pointer the GPU ROM passed in
/// x0. Phase 2 asks the VideoCore GPU for a framebuffer over the mailbox and, on success, fills the
/// framebuffer fields so `kernel_main` brings up the full GUI on HDMI; if the mailbox call fails the
/// fields stay 0 and the kernel falls back to the serial-only console (Phase 1 behaviour).
pub fn build_boot_info(dtb: u64) -> &'static mut BootInfo {
    unsafe {
        let regions = &raw const MEM_REGIONS as u64;
        let bi = &raw mut BOOT_INFO;
        (*bi).dtb_addr = dtb;
        // PI-GENET metal defect (R22 sitting 2): dtb_size was NEVER populated here — the static's 0
        // flowed to every consumer, so on metal (firmware DTB really present at `dtb`, totalsize
        // 0xd8dc observed) `genet::resolve` saw "size=0x0" and refused before parsing. Read the FDT
        // header ourselves: magic 0xd00dfeed then totalsize, both BIG-endian (the FDT spec byte
        // order — a plain u32 load would byte-swap on our little-endian cores). Bounded + honest:
        // a non-FDT pointer (QEMU raspi4b passes x0=0x100 with no blob there) fails the magic test
        // and the size stays an honest 0; an implausible totalsize (> 4 MiB) is refused too.
        if dtb != 0 {
            let hdr = dtb as *const u8;
            let be32 = |off: usize| -> u32 {
                u32::from_be_bytes([
                    hdr.add(off).read_volatile(),
                    hdr.add(off + 1).read_volatile(),
                    hdr.add(off + 2).read_volatile(),
                    hdr.add(off + 3).read_volatile(),
                ])
            };
            if be32(0) == 0xd00d_feed {
                let totalsize = be32(4) as usize;
                if totalsize >= 40 && totalsize <= 4 * 1024 * 1024 {
                    (*bi).dtb_size = totalsize;
                }
            }
        }
        (*bi).memory_regions_addr = regions;
        (*bi).memory_regions_len = 1;

        // VideoCore mailbox framebuffer. Safe to call here: mmu_init() has run, so the mailbox MMIO
        // (Device-mapped at 0xFE00B880) and the cache maintenance the driver does both work. Serial
        // is up (PL011), so the driver's diagnostics reach the Debug Probe even though fbcon — which
        // it's about to make possible — isn't online yet.
        if let Some(fb) = super::mailbox::init_framebuffer() {
            (*bi).framebuffer_addr = fb.base;
            (*bi).framebuffer_size = fb.size;
            (*bi).framebuffer_info = fb.info;
        }

        // PI-USB-1: bring up the BCM2711 PCIe root complex + the VL805 xHCI behind it. Placed HERE (end
        // of build_boot_info, with the DTB in hand) rather than mid-`kernel_main` so its gated insertion
        // shifts NO panic-location line numbers in baseline code — only the `piusb`-gated helper below
        // it moves (the knob-off byte-identity guarantee). Single-threaded (pre-SMP) and heap-free (the
        // honesty-line attach allocates no rings). CENSUS-BEFORE-TOUCH: it reads the firmware DTB for a
        // `pcie@` node FIRST and returns before ANY RC MMIO if none is present — so QEMU raspi4b (which
        // models no PCIe RC and whose DTB has no `pcie@` node) skips cleanly, never reading the unbacked
        // RC aperture (which would external-abort with the exception vectors not yet installed at this
        // pre-`kernel_main` point). `piusb`-gated: knob-off this call + the whole module vanish and
        // kernel8 is byte-identical to baseline. See arch_arm64.md §PI-USB.
        #[cfg(feature = "piusb")]
        super::piusb::bringup(dtb);

        &mut *bi
    }
}

/// PI-USB-1: map a single 1 GiB Device-nGnRnE identity block into the live EL1&0 translation regime,
/// for an MMIO window that lies OUTSIDE the fixed 0–4 GiB map `build_l1` installs. The BCM2711 PCIe
/// root complex's outbound MEM window (where the VL805 xHCI's BAR is decoded to the CPU) is placed by
/// firmware/DT at CPU-physical `0x6_0000_0000` (24 GiB) — reachable within TCR IPS=36-bit / VA=39-bit,
/// but not in the four blocks `build_l1` fills. This installs `L1[pa>>30] = device_block(...)` for the
/// 1 GiB block containing `pa`, then does the canonical set-descriptor maintenance (dsb ishst + a
/// broadcast TLBI for the block's VA + dsb ish + isb) so the mapping is live before the caller's first
/// MMIO read. Idempotent: a block already mapped Device is rewritten to the same value.
///
/// Placed at end-of-file (like the V3D call site in mailbox.rs) so its gated insertion shifts NO
/// panic-location line numbers in the code above it — the knob-off byte-identity guarantee.
///
/// SAFETY / SCOPE: `piusb`-gated (knob-off it and every call site vanish — the kernel8 image stays
/// byte-identical to baseline). Writes ONE L1 entry for a `pa` the caller controls (the RC outbound
/// window). NEVER runs in QEMU (the caller reaches it only after a live BCM2711 RC brings its link up;
/// QEMU models no RC, so the bring-up bails at the identity read long before here). The block is XN at
/// both ELs (peripheral memory is never executable). Single-threaded boot-time use (pre-SMP), so no
/// cross-core BBM concern; the broadcast TLBI is belt-and-suspenders.
#[cfg(feature = "piusb")]
pub unsafe fn map_device_1gib(pa: u64) {
    let gib = (pa >> 30) as usize;
    // 512-entry L1; a >= 512 GiB base is unreachable under the 39-bit VA — refuse rather than index OOB.
    if gib >= 512 {
        return;
    }
    let block_pa = (gib as u64) << 30;
    let l1 = &raw mut L1 as *mut u64;
    l1.add(gib).write_volatile(device_block(block_pa));
    core::arch::asm!(
        "dsb ishst",
        "tlbi vaae1is, {va}",   // invalidate the block's VA for all ASIDs (VA[55:12] in bits[43:0])
        "dsb ish",
        "isb",
        va = in(reg) block_pa >> 12,
        options(nostack, preserves_flags),
    );
}
