// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// JETSON-EL0 (M1b) — the EL0 user address-space machinery for the Jetson Orin Nano (Tegra234),
// behind `feature = "tegra_el0"`.
//
// WHAT THIS IS. `arch/aarch64/boot.rs` carries the M6a..M6d user-window/slot system that
// `arch/aarch64/syscall.rs` and `sched.rs`'s `spawn_user*` arms consume: a 4 KiB-granular EL0 window,
// per-task private translation-table branches with their own ASIDs, W^X leaf permissions, and the
// per-process framebuffer surface hole. That file is `#[cfg(feature = "baremetal")]` and is BCM2711 MMU
// code end-to-end — it owns the Pi's `L1`, builds the whole identity map, drops EL2 -> EL1 and turns the
// MMU on. None of that can be reused on the Orin, where NVIDIA's UEFI hands off with the MMU already
// running and `mmu_tegra` owns the regime. So this module RE-IMPLEMENTS the same slot system against
// the tegra EL1 regime, exposing the SAME public shape `boot.rs` exposes; `arch::aarch64::uslots`
// (mod.rs) is the one-line facade that routes `syscall.rs`/`sched.rs` at whichever of the two the
// active feature selects, so those two consumers needed no body edits beyond the module path.
//
// ── THE THREE PLACES THIS DIVERGES FROM boot.rs, AND WHY ────────────────────────────────────────────
//
// 1. THE USER WINDOW CANNOT LIVE IN GiB 0. The Pi demotes its `L1[0]` (the 0–1 GiB block) to a table so
//    it can carve EL0 permission at 4 KiB inside it; `USER_REGION` is a BSS static down there and the
//    window is identity-mapped. On Tegra234 the low 1 GiB is the DEVICE window — `mmu_tegra` maps
//    `L1[0]` Device-nGnRE because that is where UARTC (0x0C28_0000), the GIC-600 and the rest of the
//    peripheral MMIO live. Worse, `build_slot` COPIES the root table per slot and patches the user
//    entry: patching `L1[0]` would swap the kernel's own UART out from under it the moment a slot root
//    went live in `TTBR0_EL1`. So the window instead occupies an otherwise-UNUSED 1 GiB entry
//    (`USER_GIB`), and user VA != backing PA — the Pi's identity assumption is dropped, which is why
//    every kernel-side write goes through `slot_backing_ptr` (the identity-mapped PA) and never through
//    the user VA. `install` VERIFIES the chosen entry is invalid before claiming it rather than
//    asserting it in a comment.
//
// 2. THE BACKING COMES OFF THE HEAP, NOT BSS. `boot.rs` puts `USER_REGION`/`SLOT_BACKING` in BSS. Doing
//    that here would be a live bug, not a style difference: `mmu_tegra` UNMAPS the SNOC-firewalled
//    carveout windows (`carveout_holes` — the XCARVE-3/6/8/9 set), and BSS is placed by the linker
//    wherever the kernel image landed, with nothing keeping it clear of them. A backing frame inside a
//    hole has no translation at all, so the FIRST EL0 touch would fault. The heap is the allocation
//    source that is already vetted — `select_heap_region` seats it clear of every hole — so backings are
//    `alloc_zeroed`ed from it, exactly as the xHCI structures are. `carveout_overlaps` re-checks each
//    backing against the live hole set anyway and REFUSES the allocation on a hit: the heap guard is an
//    invariant of another module, and a silent dependency on it is the kind of thing that rots.
//
// 3. THE ASID GRANT ISSUES A CONSERVATIVE TLBI. `boot.rs`'s `build_slot` argues no TLBI is needed on a
//    fresh ASID (fresh tables; the previous tenant's ASID was flushed at ITS teardown, so nothing stale
//    can exist under this ASID). The argument is sound on the A72 and it is probably sound on the
//    A78AE — but "probably" is doing real work in it, the Orin has its own errata surface
//    (`mmu_tegra::a78ae_errata_probe`), and this whole chain is metal-owed and un-QEMU-able, so a
//    latent stale-TLB bug here would surface as an unreproducible EL0 fault on the bench. A single
//    broadcast `tlbi aside1is` per slot GRANT costs nothing on a path that runs once per task launch,
//    and it makes the correctness of the grant independent of the teardown argument. Deliberate; do not
//    "optimise" it away without metal evidence.
//
// ── PAN: A LOAD-BEARING PRECONDITION THAT HOLDS BY CONFIGURATION, NOT BY ABSENCE ───────────────────
//
// Several things here and in `syscall.rs` require EL1 to be able to touch EL0-accessible pages:
// `syscall::setup()` copies `USER_BLOB` in through the shared window's user VA, `sys_write` READS the
// user message buffer, and `probe_slot_isolation` does real EL1 loads of a user VA. `boot.rs` justifies
// all of that with "the PAN-less A72" — literally true there (Armv8.0 has no PAN).
//
// The Orin's Cortex-A78AE is Armv8.2 and DOES have PAN, so that justification does not transfer. The
// operations are still legal here, but for a different and more fragile reason — two configuration
// facts, both currently set in `boot_tegra`:
//   * `SPSR_EL2 = 0x3c5` at the EL2 -> EL1 `eret`. Bit 22 (PAN) is CLEAR, so the kernel lands at EL1
//     with PSTATE.PAN == 0.
//   * `SCTLR_EL1.SPAN` (bit 23) is SET in `SCTLR_EL1_VAL`, which means PSTATE.PAN is left UNCHANGED on
//     exception entry to EL1 — so the SVC from an EL0 task arrives with PAN still 0, and `sys_write`
//     can read the caller's buffer.
//
// STOP TRIPWIRE: clearing SPAN, or setting SPSR_EL2 bit 22, would turn every one of those accesses into
// a permission fault — and the failure would present as an EL0 program that "mysteriously" faults in
// the kernel, not as anything pointing at PAN. If a future arc wants PAN enforcement (a real hardening
// win), the correct move is to gate those specific accesses behind `AT S1E1RP`-style checks or explicit
// `msr PAN, #0` windows, NOT to leave this dependency implicit. Recorded here because it was previously
// written down nowhere.

// GATING. Everything here is `feature = "tegra_el0"` (which implies `tegra`). Knob off, the module is
// not compiled, the facade routes to `boot.rs`, and the jetson media is byte-identical to baseline.
//
// WHAT IS NOT HERE. No GIC group/route configuration is touched (EL0 needs none — SVC is a synchronous
// exception through the already-installed `VBAR_EL1`), and no protection is weakened: `SCTLR_EL1.WXN`
// stays as `boot_tegra` set it (compatible by construction — every leaf below is either read-only or
// execute-never at both ELs, never writable-and-executable).

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use super::mmu_tegra;

/// The EL1 boot root — `TTBR0_EL1` with the ASID field stripped, i.e. the PA of the table
/// `boot_tegra::drop_to_el1` actually installed (`mmu_tegra`'s `L1_EL1`). Latched ONCE by `install`.
///
/// Read from the LIVE system register rather than exported from `mmu_tegra`, and that is deliberate on
/// two counts. Correctness: it is the root the hardware is ACTUALLY walking, so this cannot drift from
/// what `boot_tegra` installed the way a second reference to a particular static could. Blast radius:
/// `mmu_tegra.rs` needs no edit at all — which also keeps the knob-off jetson media byte-identical,
/// since even a fully `#[cfg]`-gated 30-line append to that file was measured to move the media hash
/// (see the arc notes in arch_arm64.md §JETSON-EL0).
///
/// LATCHED, never re-read on demand: `boot_ttbr0()` is called from `teardown_user_slot` and the spawn
/// paths, where the calling core's live `TTBR0_EL1` may well be a SLOT root (with a non-zero ASID). A
/// fresh `mrs` at those sites would return that slot root and the teardown would "repoint" the core at
/// the address space it is trying to leave. `install` runs before any slot can exist, so the value it
/// latches is unambiguously the ASID-0 boot root.
static BOOT_ROOT: AtomicU64 = AtomicU64::new(0);

/// Read `TTBR0_EL1` and strip the ASID field [63:48], leaving the table PA.
#[inline]
fn live_ttbr0_root() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, TTBR0_EL1", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v & 0x0000_FFFF_FFFF_FFFF
}

// ── Geometry ────────────────────────────────────────────────────────────────────────────────────────

/// The L1 entry (1 GiB of VA) the EL0 user window occupies. `TCR_EL1.T0SZ = 25` gives a 39-bit VA, so
/// `TTBR0_EL1` spans 512 GiB and indices 0..=511 are legal.
///
/// 480 is chosen to be structurally unreachable by anything else the kernel maps, not merely unused
/// today: `mmu_tegra` maps GiB 0 (device) plus the RAM GiBs the firmware memory map reports (Orin DRAM
/// is GiB 2..=9 on an 8 GiB board), and `map_mmio_window`/`map_fb_region` map device windows
/// IDENTITY (VA == PA) under an output ceiling of 64 GiB — or 1 TiB with `pcie3`, whose highest real
/// Orin address is controller-0's ~200 GiB ECAM/`ranges`. 480 GiB is above every one of those and
/// below the 512 GiB VA ceiling. `install` still verifies the entry is invalid before claiming it.
pub const USER_GIB: usize = 480;

/// Base VA of the EL0 user window. 1 GiB-aligned, therefore 2 MiB-aligned, therefore the whole
/// `USER_STATIC_SIZE` region provably falls inside the single 2 MiB block covered by one L3 table —
/// the same structural guarantee `boot.rs` buys with `#[repr(align(0x100000))]` on its BSS anchor.
pub const USER_VA_BASE: u64 = (USER_GIB as u64) << 30;

/// The EL0 program window: 16 KiB = 4 pages. Page 0 is CODE, pages 1..3 are DATA/STACK. Identical to
/// `boot.rs` — `syscall.rs` does VA arithmetic against these and the values must agree.
pub const USER_REGION_SIZE: usize = 0x4000;
/// The CODE page(s) at the bottom of the window — the only EL0-executable memory in the system.
pub const USER_CODE_SIZE: usize = 0x1000;

// The per-process framebuffer surface hole above the program window (ELF-3 / WC-B). Values are
// byte-for-byte those of `boot.rs`: the EL0-visible VA layout must be IDENTICAL on both platforms
// because the SAME user binaries (and the same `syscall.rs` arithmetic) run against it.
//
// ORIN-TENANT (rung 6) — the CRYSTAL-HD parity fix, and the rot it removes. This module was written
// (JETSON-EL0 M1b, 2026-08-18 10:09) ten hours AFTER CRYSTAL-HD (`92435fb8`, 2026-08-18 00:23) took
// `boot.rs` and the x86 twin from 8 slots x 64 KiB / 128x128 to 4 slots x 0x51000 / 288x288 — and it
// copied the PRE-CRYSTAL-HD table, so the paragraph above was false at this module's birth. The
// divergence was not cosmetic; it was two live defects, both removed by restoring the parity the
// paragraph claims:
//   * `run /fat/VUG.ELF` on the Orin died at its first syscall: the shipped `user-vug` asks
//     `SYS_WIN_CREATE(288, 288)` (its `SW`/`SH` — CRYSTAL-HD's 288-as-committed, Peter-ruled
//     2026-08-18) and the 128 cap here answered `-EINVAL`, so the program printed
//     `:: UVUG: SYS_WIN_CREATE failed ::` and exited(1) — no EL0 program could ever own a
//     compositor window on this board.
//   * the WC-B window-verb fixture hardcodes region slot 1's surface at `base + 0x5000 + 0x51000`
//     (`syscall.rs`, `add x12, x9, #0x56, lsl #12`) — the Pi stride. Against the 0x1_0000 stride
//     here the kernel mapped slot 1's leaves at `base + 0x15000`, so the fixture's b10/b11 stores
//     aimed at RESERVED (invalid) leaves: a latent EL0 fault in every `tegra_el0` witness image.
// The slot-0 offset (`base + 0x5000`, what `SYS_FB_MAP` returns and what every shipped binary uses)
// is unchanged by this fix — only the per-slot STRIDE, the slot COUNT and the negotiable CAP move,
// and each is published per window in the RO info page rather than assumed by ring 3. `syscall.rs`
// cross-checks the parity at compile time against whichever module `uslots` selects (its WC-B const
// asserts: `FB_WIN_SLOTS <= WIN_MAX`, `FB_WIN_MAX_W * FB_WIN_MAX_H * 4 == FB_WIN_SLOT_SIZE`), and
// the asserts below hold the local arithmetic.
pub const FB_INFO_SIZE: usize = 0x1000;
pub const FB_SURFACE_W: u32 = 32;
pub const FB_SURFACE_H: u32 = 32;
pub const FB_SURFACE_STRIDE: u32 = FB_SURFACE_W * 4;
pub const FB_SURFACE_SIZE: usize = (FB_SURFACE_STRIDE * FB_SURFACE_H) as usize;
pub const FB_WIN_SLOTS: usize = 4;
pub const FB_WIN_SLOT_SIZE: usize = 0x5_1000;
pub const FB_WIN_MAX_W: u32 = 288;
pub const FB_WIN_MAX_H: u32 = 288;
pub const FB_REGION_SIZE: usize = FB_INFO_SIZE + FB_WIN_SLOTS * FB_WIN_SLOT_SIZE;

/// Total per-address-space backing: the program window + the FB hole. 0x149000 (1.29 MiB) since the
/// CRYSTAL-HD parity fix above — `boot.rs` asserts the same literal on its side. Unlike the Pi
/// (where this is 8 slots of `.bss` against a hand-placed heap floor), every backing here is
/// `alloc_zeroed`ed lazily from the 48 MiB tegra heap: 1 shared + up to `USER_SLOTS` slots =
/// 9 x 0x149000 = 11.6 MiB worst case, and a slot never claimed costs nothing.
pub const USER_STATIC_SIZE: usize = USER_REGION_SIZE + FB_REGION_SIZE;
const _: () = assert!(USER_STATIC_SIZE == 0x149000);
/// The whole region must fit the ONE per-slot L3 (512 x 4 KiB = 2 MiB), or the FB leaves would spill
/// into a table `build_slot` never wired. 0x149000 = 329 of those 512 pages. (`boot.rs` carries the
/// identical assert; the `USER_VA_BASE` GiB alignment is what guarantees non-straddling here.)
const _: () = assert!(USER_STATIC_SIZE <= 512 * 0x1000);
/// A `FB_WIN_MAX_W` x `FB_WIN_MAX_H` ARGB8888 surface must fit a window's VA slot exactly.
const _: () = assert!((FB_WIN_MAX_W * FB_WIN_MAX_H * 4) as usize == FB_WIN_SLOT_SIZE);

/// Number of per-task user address-space slots. STOP tripwire, inherited verbatim from `boot.rs`: this
/// cap is deliberate — do not raise it to satisfy a demo; a real user-memory allocator is a later arc.
pub const USER_SLOTS: usize = 8;

// ── Translation-table types and descriptors ─────────────────────────────────────────────────────────

/// One translation table: 512 entries x 8 bytes = one 4 KiB page, naturally aligned as the walker
/// requires. Kernel RAM is identity-mapped on tegra, so a static's address is its PA.
#[repr(C, align(4096))]
struct PageTable([u64; 512]);

/// The SHARED (ASID 0) user window's L2/L3 — the tegra analogue of `boot.rs`'s `L2_USER`/`L3_USER`.
/// Hung off `L1_EL1[USER_GIB]` by `install`, so the window is reachable under the BOOT root. That is
/// what lets `syscall::setup()` copy `USER_BLOB` in through the user VA at EL1 (the leaves start as
/// EL0+EL1-RW data pages), and what the M6b/M6e shared-window EL0 tasks run under.
static mut L2_USER: PageTable = PageTable([0; 512]);
static mut L3_USER: PageTable = PageTable([0; 512]);

/// Per-slot private table branches (M6d). A slot's L1 is a COPY of `L1_EL1` with `[USER_GIB]` repointed
/// at the slot's own L2 -> L3, so kernel code running while the slot root is live resolves its
/// .text/heap/stack/device mappings exactly as it does under the boot root.
static mut SLOT_L1: [PageTable; USER_SLOTS] = [const { PageTable([0; 512]) }; USER_SLOTS];
static mut SLOT_L2: [PageTable; USER_SLOTS] = [const { PageTable([0; 512]) }; USER_SLOTS];
static mut SLOT_L3: [PageTable; USER_SLOTS] = [const { PageTable([0; 512]) }; USER_SLOTS];

/// Heap PA of each slot's backing, 0 until first claimed. Allocated once per slot and never freed: a
/// slot is RECYCLED (teardown retires the ASID and mappings, `build_slot` re-scrubs the FB region for
/// the next tenant), so churning the allocator per launch would buy nothing and could fail late. This
/// is what makes `slot_backing_ptr` infallible after `alloc_user_slot` succeeds.
static SLOT_BACKING: [AtomicU64; USER_SLOTS] = [const { AtomicU64::new(0) }; USER_SLOTS];
/// Heap PA of the SHARED (ASID 0) window's backing, set by `install`.
static SHARED_BACKING: AtomicU64 = AtomicU64::new(0);
/// Whether `install` completed. `user_region`/the slot paths are meaningless before it.
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Allocation state, one flag per slot; atomic so alloc/teardown are race-free across cores.
static SLOT_USED: [AtomicBool; USER_SLOTS] = [const { AtomicBool::new(false) }; USER_SLOTS];
/// ELF-2 live-THREAD refcount per slot (several EL0 threads may share one slot's TTBR0/ASID). The slot
/// is torn down only on the 1->0 edge. Semantics identical to `boot.rs`'s.
static SLOT_REFCOUNT: [AtomicU32; USER_SLOTS] = [const { AtomicU32::new(0) }; USER_SLOTS];

// Leaf/table descriptor bits for the EL1&0 regime. Same recipe as `boot.rs` (and the same MAIR
// AttrIdx 0 = Normal Inner/Outer Write-Back that `mmu_tegra::MAIR_VAL`'s low byte 0xFF encodes), so the
// EL0-visible memory attributes are identical on both platforms.
const DESC_TABLE: u64 = 0b11; // L1/L2 table descriptor
const DESC_PAGE: u64 = 0b11; // L3 page descriptor (0b01 is INVALID at L3, unlike a block)
const DESC_AF: u64 = 1 << 10;
const SH_INNER: u64 = 0b11 << 8;
const ATTR_NORMAL: u64 = 0 << 2; // MAIR AttrIdx 0 = Normal WB
const AP_EL0: u64 = 1 << 6; // AP[7:6]=0b01 -> EL0+EL1 read-write
const AP_RO_ALL: u64 = 0b11 << 6; // AP[7:6]=0b11 -> read-only at BOTH EL1 and EL0
const DESC_PXN: u64 = 1 << 53; // privileged execute-never
const DESC_UXN: u64 = 1 << 54; // unprivileged execute-never
/// nG (bit 11) — ASID-tagged. ALL user leaves are nG so the same user VA maps different frames per
/// task with no same-VA global+non-global TLB conflict (which is CONSTRAINED UNPREDICTABLE).
const DESC_NG: u64 = 1 << 11;

/// L3 USER DATA/STACK page: EL0+EL1 RW, never executable at either EL, nG, Normal-WB. Also the state
/// every user page STARTS in, so the kernel can copy a program in before `protect_*` flips the code
/// pages. W^X half one: writable => execute-never at both ELs (so also `SCTLR_EL1.WXN`-compatible).
const fn user_data_page(pa: u64) -> u64 {
    pa | DESC_NG | DESC_UXN | DESC_PXN | AP_EL0 | DESC_AF | SH_INNER | ATTR_NORMAL | DESC_PAGE
}
/// L3 USER CODE page: read-only at BOTH ELs (AP=0b11), EL0-executable (UXN clear), EL1-non-executable
/// (PXN set), nG. W^X half two: executable => not writable at any EL. EL0 can run but not modify its
/// program; EL1 may READ it (sys_write's message bytes) but a kernel WRITE now faults, so any loader
/// must re-open the page first.
const fn user_code_page(pa: u64) -> u64 {
    pa | DESC_NG | DESC_PXN | AP_RO_ALL | DESC_AF | SH_INNER | ATTR_NORMAL | DESC_PAGE
}
/// L3 USER READ-ONLY DATA page (the FB info/geometry page): RO at both ELs, UXN|PXN, nG. The kernel
/// writes the geometry through the identity backing pointer, never this VA, so EL0 can never mutate it.
const fn user_ro_page(pa: u64) -> u64 {
    pa | DESC_NG | DESC_UXN | DESC_PXN | AP_RO_ALL | DESC_AF | SH_INNER | ATTR_NORMAL | DESC_PAGE
}
/// The RESERVED state of a user-window leaf: INVALID. On the Pi the un-mapped parts of the window hole
/// carry the boot L3's identity-RAM EL1-only descriptors (the window sits inside a real mapped GiB
/// there); here `USER_GIB` is a GiB the kernel maps NOTHING into, so the honest reserved state is a
/// fault. Strictly safer, and it is what `unmap_slot_fb_win` restores.
const RESERVED_LEAF: u64 = 0;
/// A table descriptor. Masked to bits[47:12] so no stray bits land in the table-attribute fields
/// [63:59] (NSTable/APTable/UXNTable/PXNTable); leaving those 0 adds no restriction at this level, so
/// the leaf's own AP/XN govern.
const fn table_desc(table_pa: u64) -> u64 {
    (table_pa & 0x0000_FFFF_FFFF_F000) | DESC_TABLE
}

// ── Barrier / maintenance helpers ───────────────────────────────────────────────────────────────────
//
// `TCR_EL1.IRGN0/ORGN0 = WB` and `SH0 = inner-shareable` (see `mmu_tegra::TCR_EL1_VAL`), so table walks
// are cacheable and Inner-Shareable-coherent: publishing a descriptor needs a `dsb ishst`, not a
// clean-to-PoC. (`mmu_tegra::init` cleans to PoC only because it builds its tables with the MMU OFF.)

#[inline]
fn publish() {
    unsafe { core::arch::asm!("dsb ishst", options(nostack, preserves_flags)) };
}

/// Broadcast-invalidate the TLB for one page VA, ALL ASIDs (`vaae1is`, operand `va >> 12`).
#[inline]
fn tlbi_page(va: u64) {
    unsafe {
        core::arch::asm!("tlbi vaae1is, {}", in(reg) (va >> 12), options(nostack, preserves_flags))
    };
}

#[inline]
fn sync() {
    unsafe { core::arch::asm!("dsb ish", "isb", options(nostack, preserves_flags)) };
}

// ── Install: the shared (ASID 0) window ─────────────────────────────────────────────────────────────

/// True iff `[pa, pa+len)` intersects any window `mmu_tegra` excluded from the cacheable map (the
/// XCARVE SNOC-firewalled carveout set). Such a window has NO translation, so a backing frame inside
/// one would fault on the first EL0 touch — see divergence (2) in the module header.
fn carveout_overlaps(pa: u64, len: usize) -> bool {
    let mut set = [(0u64, 0u64, 0u8); mmu_tegra::MMU_MAX_HOLES];
    let n = mmu_tegra::carveout_holes(&mut set);
    let end = pa + len as u64;
    set[..n].iter().any(|&(hb, hs, _)| hs != 0 && pa < hb + hs && hb < end)
}

/// Allocate one zeroed, page-aligned `USER_STATIC_SIZE` backing off the kernel heap, refusing (and
/// leaking nothing) if it lands in a carveout hole. Returns the PA, or 0 on failure.
fn alloc_backing() -> u64 {
    let layout = match core::alloc::Layout::from_size_align(USER_STATIC_SIZE, 0x1000) {
        Ok(l) => l,
        Err(_) => return 0,
    };
    let p = unsafe { alloc::alloc::alloc_zeroed(layout) };
    if p.is_null() {
        return 0;
    }
    let pa = p as u64;
    if carveout_overlaps(pa, USER_STATIC_SIZE) {
        // The heap guard is supposed to make this unreachable; if it ever fires, the honest move is to
        // hand the block back and fail the caller loudly rather than map an unbacked frame to EL0.
        unsafe { alloc::alloc::dealloc(p, layout) };
        serial_println!(
            ":: TEGRA-EL0: backing alloc {:#x} intersects a carveout hole — REFUSED ::",
            pa
        );
        return 0;
    }
    pa
}

/// Bring the EL0 user window up in the LIVE EL1 regime. Call ONCE, at EL1 (i.e. after
/// `boot_tegra::drop_to_el1`, whose `TTBR0_EL1` is the `L1_EL1` this hangs the window off) and after the
/// heap exists. Returns whether the window is usable; every failure prints its own reason and leaves the
/// tables untouched, so a refusal degrades to "no EL0 this boot", never to a half-installed map.
///
/// Ordering note (a real divergence from the M1b brief's map, recorded here because it is load-bearing):
/// this CANNOT run before `timer::set_not_live()`. That call sits immediately after `drop_to_el1`, and
/// the window lives in `L1_EL1`, which is not the live root until that drop — a pre-drop write through
/// the user VA would fault against the EL2 `L1`, which maps nothing at `USER_GIB`. So the call site is
/// after the drop and after `exceptions::install()` (which puts the real `VBAR_EL1` in place for the SVC
/// the EL0 task will take), and before the `run_capstone_boot_core` terminus that dispatches it.
pub fn install() -> bool {
    if INSTALLED.load(Ordering::Acquire) {
        return true;
    }
    // Latch the live EL1 root (see BOOT_ROOT) and use it as the L1 table pointer.
    let root = live_ttbr0_root();
    if root == 0 {
        serial_println!(":: TEGRA-EL0: TTBR0_EL1 is 0 — not at EL1 post-drop; user window NOT installed ::");
        return false;
    }
    BOOT_ROOT.store(root, Ordering::Release);
    let l1 = root as *mut u64;
    // (a) The chosen GiB must be genuinely free. Verified, not assumed: if some later mapping arc ever
    // grows into it, this refuses rather than silently clobbering a live kernel mapping.
    let existing = unsafe { l1.add(USER_GIB).read_volatile() };
    if existing != 0 {
        serial_println!(
            ":: TEGRA-EL0: L1_EL1[{}] already mapped ({:#x}) — user window NOT installed ::",
            USER_GIB,
            existing
        );
        return false;
    }
    // (b) The shared window's backing.
    let backing = alloc_backing();
    if backing == 0 {
        serial_println!(":: TEGRA-EL0: shared-window backing allocation FAILED ::");
        return false;
    }
    SHARED_BACKING.store(backing, Ordering::Release);

    unsafe {
        let l2 = &raw mut L2_USER as *mut u64;
        let l3 = &raw mut L3_USER as *mut u64;
        // Every leaf starts RESERVED (invalid); only the pages actually backing the window become
        // valid, and only as EL0+EL1-RW data pages for now (`syscall::setup` copies the program in
        // through the user VA at EL1; `protect_user_code` then flips the code page).
        for i in 0..512 {
            l3.add(i).write_volatile(RESERVED_LEAF);
            l2.add(i).write_volatile(RESERVED_LEAF);
        }
        let mut off = 0u64;
        while off < USER_REGION_SIZE as u64 {
            let va = USER_VA_BASE + off;
            l3.add(((va >> 12) & 0x1FF) as usize).write_volatile(user_data_page(backing + off));
            off += 0x1000;
        }
        // L2[0] -> L3 (the window is at the GiB base, so it is the first 2 MiB block), L1[GiB] -> L2.
        l2.add(0).write_volatile(table_desc(&raw const L3_USER as u64));
        publish();
        l1.add(USER_GIB).write_volatile(table_desc(&raw const L2_USER as u64));
        publish();
        // The entry was INVALID until this instant and an invalid entry is not TLB-cached, so nothing
        // stale can exist for these VAs. The flush is nevertheless issued once, here, for the same
        // reason the slot grant issues one (module header, divergence 3): it is a boot-time one-shot and
        // it makes the window's correctness independent of that argument.
        core::arch::asm!("tlbi vmalle1is", options(nostack, preserves_flags));
    }
    sync();
    INSTALLED.store(true, Ordering::Release);
    serial_println!(
        ":: TEGRA-EL0: user window installed — VA {:#x} (L1_EL1[{}]), backing PA {:#x}, {} slots ::",
        USER_VA_BASE,
        USER_GIB,
        backing,
        USER_SLOTS
    );
    true
}

// ── The public shape `syscall.rs`/`sched.rs` consume (mirrors `boot.rs` name-for-name) ──────────────

/// The EL0 user window as (base VA, size in bytes). Unlike the Pi's, this VA is NOT the backing PA —
/// kernel-side writes must go through `slot_backing_ptr`, not this address, except for the shared
/// (ASID 0) window, which IS mapped EL1-RW under the boot root so `syscall::setup` can copy through it.
pub fn user_region() -> (u64, usize) {
    (USER_VA_BASE, USER_REGION_SIZE)
}

/// The boot/shared context root: `L1_EL1 | (ASID 0 << 48)` == `L1_EL1`. Kernel tasks and the
/// shared-window EL0 tasks run under this.
pub fn boot_ttbr0() -> u64 {
    BOOT_ROOT.load(Ordering::Acquire)
}

/// ASID assigned to slot `s` (1..=USER_SLOTS; ASID 0 is the boot/shared context).
#[inline]
fn slot_asid(s: usize) -> u64 {
    (s + 1) as u64
}

/// The TTBR0 value installing slot `s`'s address space: `slot_l1_pa | (asid << 48)`.
pub fn slot_ttbr0(s: usize) -> u64 {
    debug_assert!(s < USER_SLOTS);
    let l1_pa = unsafe { (&raw const SLOT_L1).cast::<PageTable>().add(s) as u64 };
    l1_pa | (slot_asid(s) << 48)
}

/// Kernel-side identity pointer into slot `s`'s backing. The kernel copies programs and plants data
/// through THIS pointer — an EL1-RW identity mapping reachable under any root — never through the
/// ASID-tagged EL0 window VA. A78AE L1 caches are PIPT, so writes here are coherent with the EL0
/// fetch/read of the same frame at the aliased user VA.
pub fn slot_backing_ptr(s: usize) -> *mut u8 {
    debug_assert!(s < USER_SLOTS);
    SLOT_BACKING[s].load(Ordering::Acquire) as *mut u8
}

/// How many address-space slots are unclaimed right now. Reads only; never consulted on an allocation
/// path — `alloc_user_slot`'s CAS is the only thing that may decide a slot's fate, and a count taken
/// here is stale the instant it is returned.
pub fn user_slots_free() -> usize {
    (0..USER_SLOTS).filter(|&s| !SLOT_USED[s].load(Ordering::Acquire)).count()
}

/// Register one more live EL0 thread against the slot owning `asid` (a `SYS_THREAD_SPAWN` joining an
/// existing address space). Balanced by that thread's eventual `teardown_user_slot` at exit. MUST be
/// called on a live slot (refcount already >= 1) BEFORE the new thread can be dispatched.
pub fn slot_thread_retain(asid: u64) {
    debug_assert!(asid >= 1 && asid as usize <= USER_SLOTS, "retain: asid out of range");
    let prev = SLOT_REFCOUNT[(asid - 1) as usize].fetch_add(1, Ordering::AcqRel);
    debug_assert!(prev >= 1, "slot_thread_retain on a slot with no live owner");
}

/// Claim a free slot and build its private translation tables, returning the slot id. Returns `None` if
/// the pool is exhausted, or if this slot's backing could not be allocated (in which case the slot flag
/// is released again, so a failed claim leaks nothing).
pub fn alloc_user_slot() -> Option<usize> {
    if !INSTALLED.load(Ordering::Acquire) {
        return None;
    }
    for s in 0..USER_SLOTS {
        if SLOT_USED[s].compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            if SLOT_BACKING[s].load(Ordering::Acquire) == 0 {
                let pa = alloc_backing();
                if pa == 0 {
                    SLOT_USED[s].store(false, Ordering::Release); // never installed -> no TLBI owed
                    serial_println!(":: TEGRA-EL0: slot {} backing allocation FAILED ::", s);
                    return None;
                }
                SLOT_BACKING[s].store(pa, Ordering::Release);
            }
            // Seed the live-task refcount for the initial owner. The store precedes `build_slot`'s
            // publishing barrier and any dispatch onto this slot, so a `slot_thread_retain` (reachable
            // only once a task runs on the slot) always observes >= 1.
            SLOT_REFCOUNT[s].store(1, Ordering::Release);
            unsafe { build_slot(s) };
            return Some(s);
        }
    }
    None
}

/// Claim `out.len()` slots at once; return whether ALL were obtained. On a partial failure this
/// RELEASES the slots already claimed IN THIS CALL, so a multi-slot request never leaks earlier claims.
/// A slot released here was never installed in any core's `TTBR0` (a slot goes live only when a task is
/// dispatched onto it), so no core cached a translation under its ASID — clearing the flag is the whole
/// unwind, no TLBI owed. STOP tripwire: a request larger than `USER_SLOTS` must FAIL here, never grow
/// the pool.
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
                    SLOT_USED[s].store(false, Ordering::Release);
                }
                return false;
            }
        }
    }
    true
}

/// Build slot `s`'s private L1/L2/L3. The L1 is a COPY of the live `L1_EL1` (so kernel code running
/// while this root is installed resolves .text/heap/stack/device exactly as under the boot root), with
/// `[USER_GIB]` repointed at the slot's own L2 -> L3, whose leaves map the slot's OWN backing frames as
/// nG RW data pages (RW so the caller can copy the program in; `protect_user_slot_code*` then flips the
/// executable pages to their RO+X shape).
///
/// STOP TRIPWIRE — per-slot roots freeze the kernel map at COPY time. The copy is a SNAPSHOT of every
/// kernel mapping as it stood when the slot was built. Any post-boot edit to a KERNEL mapping is
/// INVISIBLE to already-built slots and MUST be mirrored into every live slot L1 (or force a rebuild +
/// TLBI). On tegra this is a SHARPER hazard than on the Pi, because `mmu_tegra` has three routines that
/// edit `L1_EL1` after boot — `map_mmio_window`, `map_fb_region` and `install_net4b_nc`. Today the EL0
/// chain is brought up at the very end of `tegra_early_stop`, after every one of those has run, so no
/// live slot can miss a window; a future arc that maps an MMIO window LATER than the first slot build
/// must mirror it. (The carveout L2 splits are safe by construction: the L1 entries pointing at
/// `L2_POOL_EL1` are copied as POINTERS, so a slot shares those tables rather than snapshotting them.)
unsafe fn build_slot(s: usize) {
    let backing = SLOT_BACKING[s].load(Ordering::Acquire);
    debug_assert!(backing != 0, "build_slot on a slot with no backing");
    let boot_l1 = BOOT_ROOT.load(Ordering::Acquire) as *const u64;
    let sl1 = unsafe { (&raw mut SLOT_L1).cast::<PageTable>().add(s).cast::<u64>() };
    let sl2 = unsafe { (&raw mut SLOT_L2).cast::<PageTable>().add(s).cast::<u64>() };
    let sl3 = unsafe { (&raw mut SLOT_L3).cast::<PageTable>().add(s).cast::<u64>() };
    unsafe {
        // Scrub the FB region before the new tenant can reach it. A slot is RECYCLED — teardown retires
        // the ASID and its mappings but never touches the backing BYTES, and a program load only writes
        // the 16 KiB program window — so without this the FB region would carry the PREVIOUS tenant's
        // frames into an address space that can map them EL0-RW. Build is the right point: it is exactly
        // the slot-recycle boundary, it runs once per tenant, and (unlike zeroing on map) it cannot wipe
        // a caller's own pixels on an idempotent re-map.
        core::ptr::write_bytes(
            (backing as *mut u8).add(USER_REGION_SIZE),
            0,
            USER_STATIC_SIZE - USER_REGION_SIZE,
        );
        for i in 0..512 {
            sl1.add(i).write_volatile(boot_l1.add(i).read_volatile());
            sl2.add(i).write_volatile(RESERVED_LEAF);
            sl3.add(i).write_volatile(RESERVED_LEAF);
        }
        // The slot's program-window leaves -> its own backing frames.
        let mut off = 0u64;
        while off < USER_REGION_SIZE as u64 {
            let va = USER_VA_BASE + off;
            sl3.add(((va >> 12) & 0x1FF) as usize).write_volatile(user_data_page(backing + off));
            off += 0x1000;
        }
        // Redirect the table branch into the slot's own L2/L3.
        sl2.add(0).write_volatile(table_desc(
            (&raw const SLOT_L3).cast::<PageTable>().add(s) as u64,
        ));
        sl1.add(USER_GIB).write_volatile(table_desc(
            (&raw const SLOT_L2).cast::<PageTable>().add(s) as u64,
        ));
        // Publish the descriptors to every Inner-Shareable walker before any core's TTBR0 walks them (a
        // slot built on the BSP is first used by a task that may run on an AP).
        publish();
        // A78AE conservatism (module header, divergence 3): flush this ASID rather than resting the
        // grant on `boot.rs`'s "fresh ASID cannot be stale" argument.
        core::arch::asm!(
            "tlbi aside1is, {asidreg}",
            asidreg = in(reg) (slot_asid(s) << 48),
            options(nostack, preserves_flags),
        );
    }
    sync();
}

/// Flip the SHARED (ASID 0) window's code page(s) `[va, va+len)` to their final EL0-RX/EL1-RO shape,
/// and probe the result with `AT`. Returns `(el0_readable, el1_write_faults)` — the M6b permission
/// proof: the page must translate for an EL0 read and REFUSE an EL1 write.
///
/// This edits the SHARED L3 in place. A slot does NOT observe the flip on its own copy (per-slot code
/// pages are protected separately via `protect_user_slot_code*`) — the same freeze rule `build_slot`
/// documents.
pub unsafe fn protect_user_code(va: u64, len: usize) -> (bool, bool) {
    debug_assert!(va & 0xFFF == 0, "protect_user_code: unaligned va");
    debug_assert!(
        va >= USER_VA_BASE && va + len as u64 <= USER_VA_BASE + USER_REGION_SIZE as u64,
        "protect_user_code: range outside the user window"
    );
    let backing = SHARED_BACKING.load(Ordering::Acquire);
    unsafe {
        let l3 = &raw mut L3_USER as *mut u64;
        let mut page = va;
        while page < va + len as u64 {
            let pa = backing + (page - USER_VA_BASE);
            l3.add(((page >> 12) & 0x1FF) as usize).write_volatile(user_code_page(pa));
            page += 0x1000;
        }
        publish();
        let mut page = va;
        while page < va + len as u64 {
            tlbi_page(page);
            page += 0x1000;
        }
        sync();

        // PAR_EL1 is per-core state clobbered by any `AT`; mask IRQs across each at->mrs pair rather
        // than resting the probe on a "no interrupt path ever executes AT" invariant nobody enforces.
        let (par_r, par_w): (u64, u64);
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
            daif = out(reg) _,
            options(nostack, preserves_flags),
        );
        ((par_r & 1) == 0, (par_w & 1) == 1)
    }
}

/// Flip slot `s`'s code page(s) at window offset 0 to their EL0-RX/EL1-RO shape (the flat-binary path).
pub unsafe fn protect_user_slot_code(s: usize, len: usize) {
    unsafe { protect_user_slot_code_range(s, 0, len) };
}

/// General form: flip the pages covering `[off, off+len)` of slot `s`'s window to CODE. The ELF loader
/// calls this once per `PT_LOAD` segment carrying `PF_X`, so an executable segment at any page offset
/// becomes code while the R/W segments stay `user_data_page`. Pages flip whole (the range is rounded
/// out to page boundaries); the loader lays code and data on DISTINCT pages, so a flip never straddles
/// a data segment.
///
/// No `AT` probe (unlike `protect_user_code`): the slot's mapping is not live under this core's current
/// TTBR0, so an `AT` here would translate the SHARED window, not this slot. The flip precedes the slot's
/// task ever running, so there is no concurrent walk under this ASID — the permission-only leaf rewrite
/// is break-before-make-exempt.
pub unsafe fn protect_user_slot_code_range(s: usize, off: usize, len: usize) {
    debug_assert!(s < USER_SLOTS);
    debug_assert!(off + len <= USER_REGION_SIZE, "protect range outside the slot window");
    let backing = SLOT_BACKING[s].load(Ordering::Acquire);
    let sl3 = unsafe { (&raw mut SLOT_L3).cast::<PageTable>().add(s).cast::<u64>() };
    let start = USER_VA_BASE + (off as u64 & !0xFFF);
    let end = USER_VA_BASE + off as u64 + len as u64;
    unsafe {
        let mut va = start;
        while va < end {
            let pa = backing + (va - USER_VA_BASE);
            sl3.add(((va >> 12) & 0x1FF) as usize).write_volatile(user_code_page(pa));
            va += 0x1000;
        }
        publish();
        let mut va = start;
        while va < end {
            tlbi_page(va);
            va += 0x1000;
        }
    }
    sync();
}

// ── The per-process framebuffer surface hole (ELF-3 / WC-B) ─────────────────────────────────────────

/// EL0 VA of the FB info page (read-only geometry), immediately above the program window. Same VA in
/// every address space; the FRAME differs per slot.
pub fn fb_info_va() -> u64 {
    USER_VA_BASE + USER_REGION_SIZE as u64
}
/// EL0 VA of the compat FB surface — identical to `fb_win_surface_va(0)`, which is what `SYS_FB_MAP`
/// returns and what existing user binaries expect.
pub fn fb_surface_va() -> u64 {
    fb_info_va() + FB_INFO_SIZE as u64
}
/// EL0 VA of window surface slot `w`.
pub fn fb_win_surface_va(w: usize) -> u64 {
    debug_assert!(w < FB_WIN_SLOTS);
    fb_info_va() + FB_INFO_SIZE as u64 + (w * FB_WIN_SLOT_SIZE) as u64
}
/// Kernel-side identity pointer to slot `s`'s FB info page (EL1-RW; the EL0 alias is read-only).
pub fn slot_fb_info_ptr(s: usize) -> *mut u8 {
    debug_assert!(s < USER_SLOTS);
    unsafe { slot_backing_ptr(s).add(USER_REGION_SIZE) }
}
/// Kernel-side identity pointer to slot `s`'s compat FB surface (== window slot 0's first page).
pub fn slot_fb_surface_ptr(s: usize) -> *mut u8 {
    debug_assert!(s < USER_SLOTS);
    unsafe { slot_backing_ptr(s).add(USER_REGION_SIZE + FB_INFO_SIZE) }
}
/// Kernel-side identity pointer to slot `s`'s window surface slot `w` (EL1-RW; the kernel composites
/// through it while EL0 draws through the aliased EL0-RW VA — PIPT caches keep the two coherent).
pub fn slot_fb_win_surface_ptr(s: usize, w: usize) -> *mut u8 {
    debug_assert!(s < USER_SLOTS && w < FB_WIN_SLOTS);
    unsafe { slot_backing_ptr(s).add(USER_REGION_SIZE + FB_INFO_SIZE + w * FB_WIN_SLOT_SIZE) }
}

/// Map slot `s`'s FB info + compat surface pages into its EL0 window (from `SYS_FB_MAP`).
pub unsafe fn map_slot_fb(s: usize) {
    debug_assert!(s < USER_SLOTS);
    unsafe { map_slot_fb_info(s) };
    unsafe { map_slot_fb_win(s, 0, FB_SURFACE_SIZE / 0x1000) };
}

/// Map slot `s`'s read-only info page alone (the half every window path shares).
///
/// IDEMPOTENT — if the leaf is ALREADY the wanted descriptor this returns without touching it. That is
/// a correctness requirement, not an optimisation, now that `SYS_WIN_CREATE` reaches this path: the
/// break-before-make below is safe only when no sibling thread is reading the page, and a window create
/// carries no such ordering. Since the descriptor is a pure function of the slot, a no-op re-map is the
/// whole fix; a leaf that DIFFERS has by definition never been a valid info page for this tenant, so
/// the BBM is unreachable by a legitimate concurrent reader.
pub unsafe fn map_slot_fb_info(s: usize) {
    debug_assert!(s < USER_SLOTS);
    let info_va = fb_info_va();
    let info_pa = slot_fb_info_ptr(s) as u64;
    let sl3 = unsafe { (&raw mut SLOT_L3).cast::<PageTable>().add(s).cast::<u64>() };
    let idx = ((info_va >> 12) & 0x1FF) as usize;
    if unsafe { sl3.add(idx).read_volatile() } == user_ro_page(info_pa) {
        return; // already mapped for this slot — no leaf edit, so no break window
    }
    unsafe {
        sl3.add(idx).write_volatile(RESERVED_LEAF); // break
        publish();
        tlbi_page(info_va);
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));
        sl3.add(idx).write_volatile(user_ro_page(info_pa)); // make
    }
    publish();
    unsafe { core::arch::asm!("isb", options(nostack, preserves_flags)) };
}

/// Map the first `pages` pages of process-slot `s`'s window surface slot `w` EL0-RW Normal-cacheable —
/// the SAME leaf shape, and therefore the same MMU attributes, the compat surface has. `pages` is the
/// NEGOTIATED page-multiple size, never the whole 16-page slot: the remainder keeps its RESERVED
/// (invalid) leaf, so a process that asked for 32x32 cannot reach the rest of its own slot, let alone
/// another's. Proper BREAK-BEFORE-MAKE, because the output address changes on a live valid leaf.
pub unsafe fn map_slot_fb_win(s: usize, w: usize, pages: usize) {
    debug_assert!(s < USER_SLOTS && w < FB_WIN_SLOTS);
    debug_assert!(pages >= 1 && pages <= FB_WIN_SLOT_SIZE / 0x1000);
    let base_va = fb_win_surface_va(w);
    let base_pa = slot_fb_win_surface_ptr(s, w) as u64;
    let sl3 = unsafe { (&raw mut SLOT_L3).cast::<PageTable>().add(s).cast::<u64>() };
    unsafe {
        for p in 0..pages {
            let va = base_va + (p * 0x1000) as u64;
            sl3.add(((va >> 12) & 0x1FF) as usize).write_volatile(RESERVED_LEAF);
        }
        publish();
        for p in 0..pages {
            tlbi_page(base_va + (p * 0x1000) as u64);
        }
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));
        for p in 0..pages {
            let va = base_va + (p * 0x1000) as u64;
            let pa = base_pa + (p * 0x1000) as u64;
            sl3.add(((va >> 12) & 0x1FF) as usize).write_volatile(user_data_page(pa));
        }
    }
    publish();
    unsafe { core::arch::asm!("isb", options(nostack, preserves_flags)) };
}

/// The `map_slot_fb_win` inverse, for `SYS_WIN_CLOSE` and ASID teardown: restore the leaves to their
/// RESERVED (invalid) state, so a closed window's surface is unreachable from EL0 the instant the TLBI
/// completes and a later re-create goes through the same break-before-make path as the first.
///
/// Unlike the map path this may run while the owner still has threads live (a close is a syscall a
/// drawing sibling cannot be ordered against), so the invalidate-then-broadcast-TLBI-then-`dsb ish`
/// ORDER is load-bearing: any concurrent EL0 access to a closed surface must FAULT, never read a stale
/// mapping. That fault is the intended fail-closed outcome, not a regression.
pub unsafe fn unmap_slot_fb_win(s: usize, w: usize, pages: usize) {
    debug_assert!(s < USER_SLOTS && w < FB_WIN_SLOTS);
    debug_assert!(pages >= 1 && pages <= FB_WIN_SLOT_SIZE / 0x1000);
    let base_va = fb_win_surface_va(w);
    let sl3 = unsafe { (&raw mut SLOT_L3).cast::<PageTable>().add(s).cast::<u64>() };
    unsafe {
        for p in 0..pages {
            let va = base_va + (p * 0x1000) as u64;
            sl3.add(((va >> 12) & 0x1FF) as usize).write_volatile(RESERVED_LEAF);
        }
        publish();
        for p in 0..pages {
            tlbi_page(base_va + (p * 0x1000) as u64);
        }
    }
    sync();
}

// ── Witness / introspection ─────────────────────────────────────────────────────────────────────────

/// Read slot `s`'s L3 leaf descriptor for the window page containing `va`. Lets the loader witness
/// assert, kernel-side, that an executable segment's page landed as a CODE leaf and a data segment's as
/// a DATA leaf — the per-segment permission proof.
pub fn slot_leaf_desc(s: usize, va: u64) -> u64 {
    debug_assert!(s < USER_SLOTS);
    let sl3 = unsafe { (&raw const SLOT_L3).cast::<PageTable>().add(s).cast::<u64>() };
    unsafe { sl3.add(((va >> 12) & 0x1FF) as usize).read_volatile() }
}

/// True iff slot `s`'s page for `va` is a CODE leaf: read-only at both ELs AND EL0-executable — the
/// exact shape `user_code_page` writes.
pub fn slot_page_is_code(s: usize, va: u64) -> bool {
    let d = slot_leaf_desc(s, va);
    (d & (0b11 << 6)) == AP_RO_ALL && (d & DESC_UXN) == 0
}

/// True iff slot `s`'s page for `va` is a DATA leaf: EL0+EL1 RW AND unprivileged-execute-never — the
/// shape `user_data_page` writes.
pub fn slot_page_is_data(s: usize, va: u64) -> bool {
    let d = slot_leaf_desc(s, va);
    (d & (0b11 << 6)) == AP_EL0 && (d & DESC_UXN) != 0
}

// ── Teardown ────────────────────────────────────────────────────────────────────────────────────────

/// Release ONE live task's hold on the slot owning `asid` (1..=USER_SLOTS) at task exit. Called for
/// EVERY slot-bound EL0 task's exit; the single-task and multi-thread cases share this path.
///
/// TWO-PHASE, order load-bearing (the exact class of bug QEMU cannot see):
///  1. ALWAYS repoint THIS core's TTBR0 off the slot root to the boot root + `isb`. This runs on every
///     thread's exit, not only the last, so no core is ever left with a torn-down (or soon-to-be
///     torn-down) slot root live in TTBR0. That is what makes the multi-core shared-ASID case sound: at
///     the final flush no OTHER core holds the root live to speculatively refill under it.
///  2. Decrement the refcount. On a NON-final release stop here — sibling threads still use the address
///     space. Only the FINAL release (1->0) broadcast-invalidates the ASID, clears the per-ASID kernel
///     state, and frees the slot.
///
/// The BACKING is deliberately not returned to the heap: slots are recycled and `build_slot` re-scrubs
/// the FB region for the next tenant, so the frames are reused rather than churned.
pub unsafe fn teardown_user_slot(asid: u64) {
    debug_assert!(asid >= 1 && asid as usize <= USER_SLOTS, "teardown: asid out of range");
    // The ASIDE1IS operand carries the ASID in Xt[63:48]; assert `asid << 48` round-trips — a
    // mis-encoded operand would flush the WRONG ASID (silent on QEMU, a stale-entry bug on metal).
    debug_assert_eq!((asid << 48) >> 48, asid, "teardown: ASID does not fit Xt[63:48]");
    let boot = boot_ttbr0(); // boot root, ASID 0
    // Phase 1 — repoint THIS core off the slot root unconditionally.
    unsafe {
        core::arch::asm!(
            "msr TTBR0_EL1, {boot}",
            "isb",
            boot = in(reg) boot,
            options(nostack, preserves_flags),
        );
    }
    // Phase 2 — only the LAST live task flushes the ASID + frees the slot.
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
    // Wipe this ASID's per-process kernel state so a future slot-reuse starts clean. Ordering is
    // load-bearing — clear BEFORE releasing the used-flag, not after: once `SLOT_USED` goes false
    // another core's `alloc_user_slot` may claim this same slot (same ASID) and begin populating its
    // row, and a clear placed after the release could wipe the NEW owner's state.
    super::syscall::clear_handle_row(asid);
    super::syscall::clear_detached(asid);
    super::syscall::clear_hidden(asid);
    SLOT_USED[(asid - 1) as usize].store(false, Ordering::Release);
}

/// Deterministic on-metal detector for the nG discipline (this arc's #1 metal risk, and the one thing
/// QEMU structurally cannot check). IRQ-masked on the calling core, it swaps TTBR0 between slot `a`'s
/// and slot `b`'s roots and does REAL EL1 loads of the SAME user VA. Real loads consult the TLB, so if
/// the user leaf were Global (an nG bug) the first load caches a global entry for the VA and the second
/// (different ASID) HITS it — returning slot `a`'s frame under both, hence `false`. With correct nG the
/// second load misses the ASID-`a` entry, re-walks slot `b`, and returns slot `b`'s frame. QEMU models
/// no TLB (it re-walks every access), so it always sees the right frame and always returns `true` —
/// which is exactly why this probe's verdict is only meaningful on silicon.
pub unsafe fn probe_slot_isolation(
    a: usize,
    b: usize,
    off: u64,
    expect_a: u64,
    expect_b: u64,
) -> bool {
    debug_assert!(a < USER_SLOTS && b < USER_SLOTS);
    // The no-TLBI TTBR0 swap is architecturally legal ONLY because the two roots carry DISTINCT ASIDs.
    // Two equal slots would share an ASID and the probe would read slot a's frame under both roots —
    // silently "passing" the isolation check it exists to make fail on a bug.
    debug_assert!(a != b, "probe_slot_isolation requires distinct slots (distinct ASIDs)");
    let va = USER_VA_BASE + off;
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
            "ldr {rb_out}, [{va}]",     // correct nG: misses ASID a, re-walks b; nG bug: hits a
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
        // Hygiene: drop the probe's cached entries for this VA across the domain.
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
