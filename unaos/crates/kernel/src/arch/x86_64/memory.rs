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


use alloc::alloc::{alloc_zeroed, Layout};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use unaos_boot_info::{BootInfo, MemoryRegion, MemoryRegionKind};

/// The UEFI memory map (as converted by the bootloader), published once at `init` so later
/// bring-up code (e.g. the SMP trampoline page check in `smp.rs`) can query it. Safe to hold for
/// the life of the kernel: the map lives in Bootloader-kind memory (LOADER_DATA/CODE), which the
/// heap picker never claims (it only takes `Usable` == CONVENTIONAL), so nothing overwrites it.
static REGIONS: spin::Once<&'static [MemoryRegion]> = spin::Once::new();

/// True iff `[addr, addr+len)` is fully covered by a single `Usable` region in the UEFI map.
/// Used to validate fixed physical addresses (the AP trampoline @ 0x8000) against the real map
/// before we trust them. Returns false if the map hasn't been published yet.
pub fn region_is_usable(addr: u64, len: u64) -> bool {
    let end = addr.saturating_add(len);
    match REGIONS.get() {
        Some(regions) => regions.iter().any(|r| {
            r.kind == MemoryRegionKind::Usable
                && r.phys_start <= addr
                && end <= r.phys_start + r.page_count * 4096
        }),
        None => false,
    }
}

pub fn init(boot_info: &'static mut BootInfo) {
    // BPACE HPACE-1: the last sub-stamp before `heap`. `mem-init d=` is everything between the start
    // of `arch::init()` and here — GDT/IDT/PIC-silence/APIC/percpu/SYSCALL-MSRs, the boot-info
    // extraction, and (on a non-witness build) the SPLASH-1 paint. `heap d=` is then this function
    // ALONE: the region scan, the diagnostics, the identity-map probe and `init_heap_raw`.
    crate::bootpace::record("mem-init");
    serial_println!(":: X86_64 Memory Init ::");

    let regions: &'static [MemoryRegion] = unsafe {
        core::slice::from_raw_parts(
            boot_info.memory_regions_addr as *const MemoryRegion,
            boot_info.memory_regions_len,
        )
    };
    REGIONS.call_once(|| regions);

    // Pick the heap region. Prefer one at/above 16 MiB so we skip the fragmented low band
    // (EBDA / legacy ROM shadow / firmware scratch) that a real UEFI map clutters with reserved
    // holes; fall back to the original "anything above 1 MiB" rule if nothing qualifies (keeps
    // QEMU and small-RAM configs working).
    let mut heap_start = 0u64;
    let mut heap_size = 0usize;
    'pick: for &min_base in &[0x0100_0000u64, 0x0010_0000u64] {
        for region in regions {
            if region.kind == MemoryRegionKind::Usable && region.phys_start >= min_base {
                let size = (region.page_count * 4096) as usize;
                if size >= crate::allocator::HEAP_SIZE {
                    heap_start = region.phys_start;
                    heap_size = crate::allocator::HEAP_SIZE;
                    break 'pick;
                }
            }
        }
    }

    if heap_size > 0 {
        // Diagnostics (serial_println! mirrors to fbcon, so these are visible on the serial-less
        // Mac): the heap choice, the low-memory layout (exposes the AP trampoline neighborhood @
        // 0x8000 and any reserved bands), total usable RAM, and an identity-map reachability probe.
        serial_println!(
            "HEAP: chose {:#x}..{:#x} ({} MiB)",
            heap_start,
            heap_start + heap_size as u64,
            heap_size / (1024 * 1024)
        );
        let mut total_usable: u64 = 0;
        for region in regions {
            if region.kind == MemoryRegionKind::Usable {
                total_usable += region.page_count * 4096;
                if region.phys_start < 0x0010_0000 {
                    serial_println!(
                        "HEAP: low Usable {:#x}..{:#x}",
                        region.phys_start,
                        region.phys_start + region.page_count * 4096
                    );
                }
            }
        }
        serial_println!("HEAP: total usable RAM {} MiB", total_usable / (1024 * 1024));

        // The xHCI rings/buffers and e1000 descriptors are allocated from this heap and handed to
        // devices as physical==bus addresses (identity map). The brief mandates DMA buffers < 4 GiB;
        // warn if the chosen window crosses 4 GiB so a 32-bit-only DMA path can't fail mysteriously.
        // In practice the first >=16 MiB Usable region is low, so this should not fire.
        if heap_start + heap_size as u64 > 0x1_0000_0000 {
            serial_println!(
                "HEAP: WARNING: heap ends above 4 GiB ({:#x}) — 32-bit-only device DMA may be unreachable.",
                heap_start + heap_size as u64
            );
        }

        // The kernel runs on the firmware's identity map (physical_memory_offset == 0), so the
        // chosen physical base must be directly addressable. Write+read a sentinel at both ends of
        // the window before handing it to the allocator: a mismatch (or a fault) localizes a bad
        // map to this labeled point instead of a mysterious later crash. The window is RAM the
        // UEFI map calls Usable; the sentinels are overwritten by the allocator's first node.
        let probe_ok = unsafe {
            let lo = heap_start as *mut u64;
            let hi = (heap_start + heap_size as u64 - 8) as *mut u64;
            core::ptr::write_volatile(lo, 0xA55A_1234_DEAD_BEEF);
            core::ptr::write_volatile(hi, 0x0BAD_F00D_5EED_C0DE);
            core::ptr::read_volatile(lo) == 0xA55A_1234_DEAD_BEEF
                && core::ptr::read_volatile(hi) == 0x0BAD_F00D_5EED_C0DE
        };
        serial_println!("HEAP: identity-map probe {}", if probe_ok { "OK" } else { "FAIL" });

        unsafe {
            crate::allocator::init_heap_raw(heap_start as *mut u8, heap_size);
        }
    } else {
        serial_println!("Available memory regions: {}", regions.len());
        for region in regions.iter().take(15) {
            serial_println!("Kind: {:?}, Start: {:#x}, Pages: {}", region.kind, region.phys_start, region.page_count);
        }
        panic!("Failed to find usable memory for heap");
    }
}

// ---------------------------------------------------------------------------------------------
// U1a: minimal 4 KiB user-page mapper.
//
// The kernel runs on the firmware's 4-level identity map (physical_memory_offset == 0), so there
// is no `OffsetPageTable` / frame allocator to lean on. To hand ring 3 a permission-split window
// we walk the live CR3 tables directly. Two facts make this cheap and safe:
//   * identity map => a page-table frame's physical address equals its (virtual) address, so a
//     heap `alloc_zeroed(4 KiB, 4 KiB)` doubles as a frame we can both write through and name in a
//     parent entry; and
//   * the window sits at a FRESH top-level slot (`syscall::USER_BASE` = 1 TiB = PML4[2]), so the
//     only pre-existing table we edit is the PML4 (one new entry) — every lower table is newly
//     allocated from the heap. `translate` proves each target page is unmapped before we map it.
// This is the x86 analogue of the aarch64 identity-mapped USER_REGION; the security boundary is
// the ring-3 (USER_BASE) mapping's per-page U / W / NX bits, not the kernel's identity alias.
// ---------------------------------------------------------------------------------------------

const PTE_PRESENT: u64 = 1 << 0;
const PTE_WRITABLE: u64 = 1 << 1;
const PTE_USER: u64 = 1 << 2;
const PTE_HUGE: u64 = 1 << 7;
const PTE_NX: u64 = 1 << 63;
/// Physical-address field of a page-table entry (bits 51:12).
const PTE_ADDR: u64 = 0x000F_FFFF_FFFF_F000;

// --- memory-type selector bits (PAT index = PAT:PCD:PWT) -------------------------------------
// A leaf's memory type is chosen by a 3-bit index into IA32_PAT, assembled from three PTE bits.
// PCD and PWT are always bits 4 and 3; the PAT bit MOVES with the leaf level, which is the whole
// reason the two constants below are distinct and must never be used at the wrong level:
//   * 4 KiB PTE  -> PAT is bit 7 (bit 12 is address);
//   * 2 MiB / 1 GiB leaf -> PAT is bit 12 (bit 7 is the HUGE flag, hence `PTE_PAT_4K == PTE_HUGE`
//     numerically — the same bit position means two different things at two different levels).
const PTE_PWT: u64 = 1 << 3;
const PTE_PCD: u64 = 1 << 4;
const PTE_PAT_4K: u64 = 1 << 7;
const PTE_PAT_HUGE: u64 = 1 << 12;

/// The PAT bit for a leaf at `level` (1 = 4 KiB, 2 = 2 MiB, 3 = 1 GiB).
#[inline]
fn pat_bit_for_level(level: u8) -> u64 {
    if level == 1 { PTE_PAT_4K } else { PTE_PAT_HUGE }
}

#[inline]
fn pml4_index(va: u64) -> usize { ((va >> 39) & 0x1FF) as usize }
#[inline]
fn pdpt_index(va: u64) -> usize { ((va >> 30) & 0x1FF) as usize }
#[inline]
fn pd_index(va: u64) -> usize { ((va >> 21) & 0x1FF) as usize }
#[inline]
fn pt_index(va: u64) -> usize { ((va >> 12) & 0x1FF) as usize }

/// Root of the live page-table hierarchy (identity-mapped, so the physical CR3 value is directly
/// addressable).
fn cr3_table() -> *mut u64 {
    x86_64::registers::control::Cr3::read().0.start_address().as_u64() as *mut u64
}

/// Walk the live CR3 tables and return the physical address `va` maps to, or `None` if any level
/// on the path is not present. Honors 1 GiB / 2 MiB huge pages. Read-only — used to prove the user
/// window is unmapped before we create it (so USER_BASE can never silently alias kernel memory).
pub fn translate(va: u64) -> Option<u64> {
    unsafe {
        let pml4e = *cr3_table().add(pml4_index(va));
        if pml4e & PTE_PRESENT == 0 {
            return None;
        }
        let pdpte = *((pml4e & PTE_ADDR) as *const u64).add(pdpt_index(va));
        if pdpte & PTE_PRESENT == 0 {
            return None;
        }
        if pdpte & PTE_HUGE != 0 {
            return Some((pdpte & PTE_ADDR) | (va & 0x3FFF_FFFF));
        }
        let pde = *((pdpte & PTE_ADDR) as *const u64).add(pd_index(va));
        if pde & PTE_PRESENT == 0 {
            return None;
        }
        if pde & PTE_HUGE != 0 {
            return Some((pde & PTE_ADDR) | (va & 0x1F_FFFF));
        }
        let pte = *((pde & PTE_ADDR) as *const u64).add(pt_index(va));
        if pte & PTE_PRESENT == 0 {
            return None;
        }
        Some((pte & PTE_ADDR) | (va & 0xFFF))
    }
}

/// Run `f` with CR0.WP momentarily cleared, so supervisor writes to the firmware's read-only
/// page-table pages land. The firmware identity map marks its PML4/PDPT/... pages read-only and
/// CR0.WP=1, so writing our new PML4 entry #PFs (PROTECTION_VIOLATION) otherwise. This is the
/// standard page-table-edit sequence — it does NOT touch any arc protection (SMEP/NXE live in
/// CR4/EFER, per-page U/W/NX live in the PTEs); WP governs only whether ring 0 honours the RO bit,
/// and it is restored before returning. Interrupts are held off across the window so nothing else
/// runs while WP is clear.
pub fn with_page_tables_writable<F: FnOnce()>(f: F) {
    use x86_64::registers::control::Cr0;
    const CR0_WP: u64 = 1 << 16;
    crate::arch::without_interrupts(|| {
        let cr0 = Cr0::read_raw();
        let had_wp = cr0 & CR0_WP != 0;
        if had_wp {
            unsafe { Cr0::write_raw(cr0 & !CR0_WP) };
        }
        f();
        if had_wp {
            unsafe { Cr0::write_raw(cr0) };
        }
    });
}

/// Allocate one zeroed, 4 KiB-aligned page from the identity-mapped heap; return its physical
/// address (== its virtual address, offset 0). Deliberately leaked: it becomes a live page-table
/// frame or a user page for the whole life of the kernel.
pub fn alloc_page_frame() -> u64 {
    let layout = Layout::from_size_align(4096, 4096).expect("U1a: bad page-frame layout");
    let p = unsafe { alloc_zeroed(layout) };
    assert!(!p.is_null(), "U1a: out of memory for a user page frame");
    p as u64
}

/// Descend one page-table level, creating the next table (PRESENT|WRITABLE|USER) if the entry is
/// empty. The USER bit on EVERY intermediate level is mandatory: ring-3 reachability is the AND of
/// the U/S bits along the whole path (a supervisor-only parent would make the leaf inaccessible to
/// ring 3). WRITABLE on a parent does not make leaves writable — the leaf's own W bit governs that,
/// so the code page can still be dropped read-only below. Panics on a huge-page slot (our fresh
/// PML4[2] window never overlaps one).
unsafe fn next_table(entry: *mut u64) -> *mut u64 {
    let e = *entry;
    if e & PTE_PRESENT == 0 {
        let frame = alloc_page_frame();
        *entry = (frame & PTE_ADDR) | PTE_PRESENT | PTE_WRITABLE | PTE_USER;
        frame as *mut u64
    } else {
        assert!(e & PTE_HUGE == 0, "U1a: user window overlaps a huge page");
        (e & PTE_ADDR) as *mut u64
    }
}

/// Map a single fresh 4 KiB user page `va -> phys`, leaf flags PRESENT|USER plus `writable` / `nx`
/// as requested. Asserts the leaf was previously empty (caller must `translate` it first). NX
/// requires EFER.NXE, enabled in `syscall::init` before any mapping.
pub unsafe fn map_user_page(va: u64, phys: u64, writable: bool, nx: bool) {
    let pdpt = next_table(cr3_table().add(pml4_index(va)));
    let pd = next_table(pdpt.add(pdpt_index(va)));
    let pt = next_table(pd.add(pd_index(va)));
    let pte = pt.add(pt_index(va));
    assert!(*pte & PTE_PRESENT == 0, "U1a: user page already mapped");
    let mut flags = PTE_PRESENT | PTE_USER;
    if writable {
        flags |= PTE_WRITABLE;
    }
    if nx {
        flags |= PTE_NX;
    }
    *pte = (phys & PTE_ADDR) | flags;
}

// U1b B4: the user code page is now mapped READ-ONLY at USER_BASE from the start (`syscall::setup`
// passes `writable=false`), so there is no writable→read-only "flip" and thus no cross-core stale
// writable mapping to shoot down. The blob is copied through the identity alias (a distinct kernel
// VA), never through USER_BASE, so the ring-3 mapping never needs the WRITABLE bit. The old
// `protect_user_page_ro` (a BSP-only `invlpg` after a live W-drop) is gone with the hole it left: a
// PTE that was never writable cannot be cached writable on any core. W^X is enforced across cores
// by construction — see `docs/SECURITY.md`.

// =================================================================================================
// U3 — per-process address spaces (CR3). The x86 mirror of aarch64 M6d (per-task TTBR0/ASID).
// -------------------------------------------------------------------------------------------------
// Until U3, every ring-3 task (U1a/U1b/U2) shared ONE user window at USER_BASE=1 TiB in the single
// firmware page table. U3 gives each user PROCESS its own top-level page table (its own CR3): each
// slot's PML4 SHARES the kernel half (identity map incl. high MMIO like the xHCI BAR at 768 GiB,
// heap, kernel code/stacks, IST/percpu/GDT — every PML4 entry EXCEPT PML4[2]) by copying those
// entries from the live kernel PML4 (so they point at the SAME kernel next-level tables — a kernel
// mapping edit BELOW the PML4 propagates automatically, unlike a deep copy), and builds its OWN
// USER_BASE window in PML4[2]. Two processes can then map the same VA to different frames.
//
// PLAIN CR3, no PCID: a `mov cr3` flushes the non-global TLB, which is exactly the isolation we want
// (user pages are never global), so no explicit invalidation is needed on a switch — the CR3 write
// is the flush. PCID (the x86 analogue of M6d's ASID optimization) is DEFERRED; correct, and there
// are only a handful of switches per boot. Ring 3 is cooperative (RFLAGS.IF clear — not yet
// preemptible), so a task runs from its trampoline CR3-load to its exit under one CR3, and a
// syscall/fault runs under the caller's own CR3 (correct: copy_from_user reads that process's
// window) — CR3 only moves at trampoline-entry and exit-teardown, mirroring M6d's cooperative
// lifecycle. When preemptible ring 3 lands, the CR3 switch moves to the general dispatch path.
//
// STATIC POOL (mirrors M6d's boot.rs 8-slot pool): no heap, a hard cap that is a STOP tripwire, and
// teardown = flip a flag (the tables are rebuilt on the next alloc; the restoring `mov cr3`
// full-flush retires the retired process's user TLB entries). A real user-memory allocator (dynamic
// slots, paged user memory) is a later arc.
//
// PER-SLOT TABLE FREEZE (STOP tripwire, mirrors M6d): a slot copies the kernel PML4 ENTRIES at build
// time. Because those entries point at the SHARED kernel next-level tables, any kernel edit below
// the PML4 is visible to live slots — but a NEW top-level (PML4-entry) kernel region added post-boot
// would be INVISIBLE to already-built slots. The kernel never adds a PML4 entry post-boot (the
// identity map is fixed at boot; the only per-process PML4 entry is [2]). If that ever changes, the
// new region must be mirrored into every live slot PML4 (or force a rebuild).

/// Number of concurrent per-process address spaces. Hard cap — a STOP tripwire, like M6d's 8.
pub const USER_SLOTS: usize = 8;
/// Pages in a user window: code, data, and two stack pages. MUST match `syscall::USER_WINDOW_PAGES`.
const U3_WINDOW_PAGES: usize = 4;
pub const PAGE_4K: u64 = 0x1000;

/// A 4 KiB-aligned page-table (512 × u64). In `.bss` (identity-mapped), so its address IS its
/// physical address — usable directly as a CR3 / next-level pointer.
#[repr(C, align(4096))]
struct PageTable([u64; 512]);
impl PageTable {
    const fn zeroed() -> Self {
        PageTable([0; 512])
    }
}

// =================================================================================================
// WINX-1 — the per-process off-screen FRAMEBUFFER REGION (SYS_WIN_CREATE / SYS_WIN_PRESENT).
//
// The x86 twin of aarch64 `boot.rs`'s ELF-3/WC-B FB hole, with the SAME VA layout, because the layout
// is part of the window ABI: a ring-3 program reads its surface at a FIXED offset from its own window
// base, and `crates/user-stat` / `crates/user-vug` hardcode `base + 0x5000`. Layout in the hole
// immediately above the 16 KiB program window:
//
//     [+0x4000]                          the RO info page (1 page) — geometry the app reads
//     [+0x5000 + w * FB_WIN_SLOT_SIZE]   window `w`'s RW surface slot (16 pages), w in 0..FB_WIN_SLOTS
//
// Ring 3 NEVER gets the real scan-out: it owns only these off-screen surface bytes, and the kernel
// composites them through SYS_WIN_PRESENT. A surface is negotiated at create time and only its
// PAGE-MULTIPLE size is actually mapped — the rest of the 64 KiB slot stays UNMAPPED (leaf 0), so
// nothing beyond the negotiated surface is reachable from ring 3.
//
// THE PROGRAM WINDOW IS UNCHANGED. `U3_WINDOW_PAGES` still means exactly what it always meant — the 4
// pages of code + data + 2 stack that `build_slot` maps eagerly and that `syscall::USER_WINDOW_PAGES`
// mirrors. The FB region is a SEPARATE, ADDITIONAL range mapped lazily. That is deliberate: it means
// the two `USER_WINDOW_PAGES` consumers in `syscall.rs` — `record_ring3_kill`'s expected-fault window
// and `user_range_ok`'s syscall-buffer bound — keep their current meaning with no edit and no audit
// hole. A fault in the FB hole is correctly NOT the expected U1b fault, and a surface VA is correctly
// NOT a legal syscall buffer (the fail-closed direction, and what aarch64's `user_range_ok` also does).
//
// ONE PT STILL SUFFICES. The slot's single PT covers 512 pages = 2 MiB from `USER_BASE`, and the whole
// region is `0x85000` = 133 pages, so the FB leaves land in `SLOT_PT[s]` alongside the program window
// — no new table level, and `build_slot`'s PML4[2]→PDPT[0]→PD[0]→PT wiring is untouched.

/// WINX-1: the read-only geometry page (1 page), at `USER_BASE + 0x4000`.
pub const FB_INFO_SIZE: usize = 0x1000;
/// WINX-1: window surface slots per address space. Matches the compositor's fixed window table
/// (`video::wm::MAX_WINDOWS`) and `syscall::WIN_MAX`. STOP tripwire: like `USER_SLOTS` this cap is
/// deliberate — do not raise it for a demo.
pub const FB_WIN_SLOTS: usize = 8;
/// WINX-1: VA reserved per window surface slot — 64 KiB = 16 pages = a 128x128 ARGB8888 surface.
pub const FB_WIN_SLOT_SIZE: usize = 0x1_0000;
/// WINX-1: the largest surface edge a window may negotiate (128 * 128 * 4 == `FB_WIN_SLOT_SIZE`).
pub const FB_WIN_MAX_W: u32 = 128;
pub const FB_WIN_MAX_H: u32 = 128;
/// WINX-1: the whole FB hole reserved above the program window.
pub const FB_REGION_SIZE: usize = FB_INFO_SIZE + FB_WIN_SLOTS * FB_WIN_SLOT_SIZE; // 0x81000
/// WINX-1: byte offset of the info page from the slot's window base.
pub const FB_INFO_OFF: usize = U3_WINDOW_PAGES * 4096; // 0x4000
/// WINX-1: byte offset of window surface slot 0 from the slot's window base.
pub const FB_SURFACE_OFF: usize = FB_INFO_OFF + FB_INFO_SIZE; // 0x5000

/// WINX-1: total per-slot backing — the 16 KiB program window plus the FB hole. Mirrors aarch64's
/// `USER_STATIC_SIZE` (0x85000). 8 slots of this is ~4.25 MiB of `.bss`, the price of a static pool
/// with no user-memory allocator; a real allocator is the same later arc `USER_SLOTS` is waiting on.
const USER_STATIC_SIZE: usize = U3_WINDOW_PAGES * 4096 + FB_REGION_SIZE;
const _: () = assert!(USER_STATIC_SIZE == 0x85000);
/// The whole region must fit the ONE per-slot PT (512 * 4 KiB = 2 MiB), or the FB leaves would spill
/// into a page table `build_slot` never wired.
const _: () = assert!(USER_STATIC_SIZE <= 512 * 4096);
/// A 128x128 ARGB8888 surface must fit a window's VA slot exactly.
const _: () = assert!((FB_WIN_MAX_W * FB_WIN_MAX_H * 4) as usize == FB_WIN_SLOT_SIZE);

/// A slot's user backing store: the `U3_WINDOW_PAGES` program frames (code + data + 2 stack) followed
/// by the FB region's frames. Page-aligned, and `USER_STATIC_SIZE` is itself a page multiple, so the
/// array stride keeps every slot's frames page-aligned.
#[repr(C, align(4096))]
struct Backing([u8; USER_STATIC_SIZE]);
impl Backing {
    const fn zeroed() -> Self {
        Backing([0; USER_STATIC_SIZE])
    }
}

// One PML4 + one PDPT + one PD + one PT per slot: the USER_BASE window only ever touches PML4[2] →
// PDPT[0] → PD[0] → PT[0..U3_WINDOW_PAGES], so a single next-level table at each level suffices.
static mut SLOT_PML4: [PageTable; USER_SLOTS] = [const { PageTable::zeroed() }; USER_SLOTS];
static mut SLOT_PDPT: [PageTable; USER_SLOTS] = [const { PageTable::zeroed() }; USER_SLOTS];
static mut SLOT_PD: [PageTable; USER_SLOTS] = [const { PageTable::zeroed() }; USER_SLOTS];
static mut SLOT_PT: [PageTable; USER_SLOTS] = [const { PageTable::zeroed() }; USER_SLOTS];
static mut SLOT_BACKING: [Backing; USER_SLOTS] = [const { Backing::zeroed() }; USER_SLOTS];
static SLOT_USED: [AtomicBool; USER_SLOTS] = [const { AtomicBool::new(false) }; USER_SLOTS];

/// The kernel (firmware) PML4 physical base, captured once on the BSP at boot BEFORE any process CR3
/// is installed. Restoring it on task teardown returns the CPU to the shared kernel address space.
static KERNEL_CR3: AtomicU64 = AtomicU64::new(0);

/// Physical (== virtual) base of a static slot table.
#[inline]
fn table_pa(p: *const u64) -> u64 {
    p as u64
}
#[inline]
fn slot_pml4_ptr(s: usize) -> *mut u64 {
    unsafe { (&raw mut SLOT_PML4[s]).cast::<u64>() }
}
#[inline]
fn slot_pdpt_ptr(s: usize) -> *mut u64 {
    unsafe { (&raw mut SLOT_PDPT[s]).cast::<u64>() }
}
#[inline]
fn slot_pd_ptr(s: usize) -> *mut u64 {
    unsafe { (&raw mut SLOT_PD[s]).cast::<u64>() }
}
#[inline]
fn slot_pt_ptr(s: usize) -> *mut u64 {
    unsafe { (&raw mut SLOT_PT[s]).cast::<u64>() }
}

/// Kernel identity pointer to slot `s`'s user backing — write a loaded program / plant a sentinel
/// through THIS, never through USER_BASE, so the process code mapping stays read-only (W^X holds).
pub fn slot_backing_ptr(s: usize) -> *mut u8 {
    unsafe { (&raw mut SLOT_BACKING[s]).cast::<u8>() }
}

/// The per-process CR3 value (its PML4 physical base) for slot `s`.
pub fn slot_cr3(s: usize) -> u64 {
    slot_pml4_ptr(s) as u64
}

// -------------------------------------------------------------------------------------------------
// WINX-1 — FB region accessors + lazy mapping.
//
// The KERNEL identity pointers below are how the compositor and the info-page writer touch these
// bytes: the backing is `.bss`, so its address IS its physical address and its kernel VA. Ring 3 sees the
// same frames at `USER_BASE + offset` in its own address space, and only after the matching `map_*`
// call. Nothing here reads or writes through the ring-3 VA.
//
// WHY WRITE THE PT DIRECTLY rather than reuse `map_user_page`: that helper walks from the LIVE CR3
// (so it can only map the currently-installed slot) and asserts the leaf was empty (so it cannot be
// idempotent). Both are wrong here — a window verb may run for a slot while a different CR3 is live,
// and a re-create of region slot 0 must be able to re-install the same leaves. Writing `SLOT_PT[s]`
// through its identity pointer is the same thing `build_slot` does, and reaches any slot from any
// context.

/// WINX-2: apply ring-3 page permissions to `[off, off + len)` of slot `s`'s PROGRAM window — the ELF
/// loader's per-segment W^X application. Every page the range touches (partial pages included: a page is
/// the permission granularity, so a segment that ends mid-page still owns that whole page) is rewritten
/// to `PRESENT | USER`, plus `WRITABLE` iff `writable` and `NX` iff `!exec`.
///
/// The leaves already exist — `build_slot` mapped all four program pages with its default shape (page 0
/// RX-RO, pages 1..N RW+NX) — so this is a permission CHANGE on live entries, not a fresh mapping. x86
/// permits that directly followed by `invlpg`; there is no break-before-make requirement (the aarch64
/// twin's `protect_user_slot_code_range` must BBM, which is the only structural difference).
///
/// W^X is the CALLER's invariant to preserve, and it does: `validate_elf` rejects any segment that is
/// both writable and executable before this is ever reached, so no call can produce a W+X page. The
/// assertion here is a second, local line of defence rather than the primary one.
///
/// Panics if the range escapes the program window — a segment that large is rejected by `validate_elf`
/// (`segment overflows the slot window`), so reaching here with one is a kernel bug, not bad input.
pub unsafe fn protect_user_slot_range(
    s: usize,
    off: usize,
    len: usize,
    writable: bool,
    exec: bool,
) {
    assert!(s < USER_SLOTS, "protect_user_slot_range: slot out of range");
    assert!(!(writable && exec), "protect_user_slot_range: W^X violation");
    let win = U3_WINDOW_PAGES * 4096;
    let end = off.checked_add(len).expect("protect_user_slot_range: range overflow");
    assert!(end <= win, "protect_user_slot_range: range escapes the program window");
    if len == 0 {
        return;
    }
    let first = off / 4096;
    let last = (end - 1) / 4096;
    let mut flags = PTE_PRESENT | PTE_USER;
    if writable {
        flags |= PTE_WRITABLE;
    }
    if !exec {
        flags |= PTE_NX;
    }
    for p in first..=last {
        let va = super::syscall::USER_BASE + (p as u64) * PAGE_4K;
        let frame = slot_backing_ptr(s) as u64 + (p as u64) * PAGE_4K;
        unsafe {
            *slot_pt_ptr(s).add(pt_index(va)) = (frame & PTE_ADDR) | flags;
            invlpg(va);
        }
    }
}

/// WINX-1: kernel identity pointer to slot `s`'s RO info page.
pub fn slot_fb_info_ptr(s: usize) -> *mut u8 {
    debug_assert!(s < USER_SLOTS);
    unsafe { slot_backing_ptr(s).add(FB_INFO_OFF) }
}

/// WINX-1: kernel identity pointer to slot `s`'s window surface slot `w`.
pub fn slot_fb_win_surface_ptr(s: usize, w: usize) -> *mut u8 {
    debug_assert!(s < USER_SLOTS && w < FB_WIN_SLOTS);
    unsafe { slot_backing_ptr(s).add(FB_SURFACE_OFF + w * FB_WIN_SLOT_SIZE) }
}

/// WINX-1: the ring-3 VA of window surface slot `w`. The SAME VA in every process slot; the FRAME differs
/// per process (its own backing), installed by `map_slot_fb_win`. This is the VA a ring-3 program
/// computes as `its own window base + 0x5000 + w * 0x10000`.
pub fn fb_win_surface_va(w: usize) -> u64 {
    debug_assert!(w < FB_WIN_SLOTS);
    super::syscall::USER_BASE + (FB_SURFACE_OFF + w * FB_WIN_SLOT_SIZE) as u64
}

/// WINX-1: install one leaf of slot `s`'s FB region at byte offset `off` from the window base.
/// `writable` distinguishes the RW surface pages from the RO info page. Always NX — nothing in the FB
/// region is code, and a surface ring 3 could execute would be a W^X hole by construction.
///
/// IDEMPOTENT by design (a re-create of the same region slot re-installs identical leaves). The `invlpg`
/// is unconditional and cheap: mapping a previously-NOT-PRESENT leaf needs no shootdown on x86 (a
/// non-present translation is never cached), and re-mapping an identical leaf is a no-op, so this only
/// has to cover the case where a leaf's flags changed. Cross-core staleness is not reachable here: a
/// slot's user leaves are only ever touched by a window verb running in that slot, or by teardown, which
/// reloads CR3 (a full non-global flush) before the slot can be reused.
unsafe fn map_slot_fb_page(s: usize, off: usize, writable: bool) {
    debug_assert!(s < USER_SLOTS && off + 4096 <= USER_STATIC_SIZE);
    let va = super::syscall::USER_BASE + off as u64;
    let frame = slot_backing_ptr(s) as u64 + off as u64;
    let mut flags = PTE_PRESENT | PTE_USER | PTE_NX;
    if writable {
        flags |= PTE_WRITABLE;
    }
    unsafe {
        *slot_pt_ptr(s).add(pt_index(va)) = (frame & PTE_ADDR) | flags;
        invlpg(va);
    }
}

/// WINX-1: map slot `s`'s RO info page (ring-3 read-only — the kernel publishes geometry there and ring-3
/// must not be able to forge it; the kernel writes through the identity alias, never this leaf).
/// Idempotent — every window create calls it.
pub unsafe fn map_slot_fb_info(s: usize) {
    unsafe { map_slot_fb_page(s, FB_INFO_OFF, false) };
}

/// WINX-1: map exactly `pages` pages of slot `s`'s window surface slot `w` (ring-3 RW, NX). Only the
/// NEGOTIATED page-multiple size is mapped; the rest of the 64 KiB VA slot stays unmapped, so a program
/// that walks past its own surface takes a contained ring-3 fault instead of reading a neighbour window.
pub unsafe fn map_slot_fb_win(s: usize, w: usize, pages: usize) {
    debug_assert!(w < FB_WIN_SLOTS && pages <= FB_WIN_SLOT_SIZE / 4096);
    let base = FB_SURFACE_OFF + w * FB_WIN_SLOT_SIZE;
    for p in 0..pages {
        unsafe { map_slot_fb_page(s, base + p * 4096, true) };
    }
}

/// WINX-1: the `map_slot_fb_win` inverse, for window close and slot teardown. Clears the leaves back to
/// NOT-PRESENT and shoots them down — this direction DOES need the `invlpg`, because the mapping was
/// live and is cached. The backing bytes are left as they are; `build_slot`'s next tenant zeroes what it
/// maps, and an unmapped frame is unreachable from ring 3 regardless.
pub unsafe fn unmap_slot_fb_win(s: usize, w: usize, pages: usize) {
    debug_assert!(s < USER_SLOTS && w < FB_WIN_SLOTS && pages <= FB_WIN_SLOT_SIZE / 4096);
    let base = FB_SURFACE_OFF + w * FB_WIN_SLOT_SIZE;
    for p in 0..pages {
        let va = super::syscall::USER_BASE + (base + p * 4096) as u64;
        unsafe {
            *slot_pt_ptr(s).add(pt_index(va)) = 0;
            invlpg(va);
        }
    }
}

/// WINX-1: drop every FB leaf of slot `s` — the info page and all window surface slots — and zero the
/// region's backing bytes. Called from slot teardown so a RECYCLED slot can never inherit the previous
/// tenant's window pixels or a stale geometry page. `build_slot` only rebuilds the 16 KiB program
/// window, so without this a fresh tenant's first `SYS_WIN_CREATE` would map a slot still holding the
/// last tenant's frame.
pub unsafe fn clear_slot_fb(s: usize) {
    debug_assert!(s < USER_SLOTS);
    for w in 0..FB_WIN_SLOTS {
        unsafe { unmap_slot_fb_win(s, w, FB_WIN_SLOT_SIZE / 4096) };
    }
    let info_va = super::syscall::USER_BASE + FB_INFO_OFF as u64;
    unsafe {
        *slot_pt_ptr(s).add(pt_index(info_va)) = 0;
        invlpg(info_va);
        core::ptr::write_bytes(slot_backing_ptr(s).add(FB_INFO_OFF), 0, FB_REGION_SIZE);
    }
}

/// U4x: the address-space SLOT the caller is currently running in, matched from the LIVE CR3 against
/// the slot pool — the x86 twin of aarch64's `current_asid` (which reads `TTBR0_EL1[63:48]`). x86 has
/// no architectural ASID, so a per-process handle table is keyed by this slot index instead. A syscall
/// runs with the caller's process CR3 still live (the kernel half is shared into every slot PML4, so
/// the handler runs fine under it — CR3 is only restored at task teardown), so the live CR3 names the
/// CALLER's address space. `None` when the live CR3 is the shared kernel window (`user_cr3 == 0` —
/// U1a/U2's tasks): such a caller has no private slot and therefore no handle table. No PCID
/// (CR4.PCIDE is off), so the CR3 base compares directly against `slot_cr3`.
pub fn current_slot() -> Option<usize> {
    let live = cr3_table() as u64;
    (0..USER_SLOTS).find(|&s| slot_cr3(s) == live)
}

/// Capture (once) and return the kernel PML4 physical base. First call MUST be on the BSP at boot
/// while the firmware/kernel CR3 is live (before any process CR3 switch) — U3 setup guarantees this.
pub fn kernel_cr3() -> u64 {
    let v = KERNEL_CR3.load(Ordering::Relaxed);
    if v != 0 {
        return v;
    }
    let cur = cr3_table() as u64;
    // Race-safe: the first writer wins; all writers observe the same live kernel CR3 at boot.
    KERNEL_CR3.store(cur, Ordering::Relaxed);
    cur
}

/// Load `cr3` into the CR3 register (installs that address space; flushes the non-global TLB).
#[inline]
pub unsafe fn load_cr3(cr3: u64) {
    unsafe { core::arch::asm!("mov cr3, {}", in(reg) cr3, options(nostack, preserves_flags)) };
}

/// Restore the shared kernel address space (used on user-task teardown before freeing the slot).
#[inline]
pub fn restore_kernel_cr3() {
    unsafe { load_cr3(kernel_cr3()) };
}

/// U3.5: install `target` CR3 iff it differs from the live one. A `mov cr3` is a full non-global TLB
/// flush, so the "only if different" skips the redundant flush on the common no-switch dispatch (same
/// task resumed, or kernel task → kernel task). Called IRQ-masked from the scheduler DISPATCH path,
/// before switching into the incoming task — the single site where a task's address space is
/// established for BOTH first entry (was the trampoline) and resume-after-preemption (which never
/// goes through the trampoline). `target` is a raw CR3 base (no PCID — CR4.PCIDE is off), directly
/// comparable to the live CR3 base.
#[inline]
pub unsafe fn switch_cr3_if_needed(target: u64) {
    if cr3_table() as u64 != target {
        unsafe { load_cr3(target) };
    }
}

/// Build slot `s`'s page table: share the kernel half (copy every live PML4 entry except PML4[2]),
/// then wire the slot's own PML4[2] → PDPT[0] → PD[0] → PT[0..N] → its backing frames with the U1a
/// window shape (page 0 code = USER + RX, read-only from the start; pages 1..N data/stack = USER +
/// RW + NX). The code page is RO-from-start (no writable→RO flip), so W^X holds by construction.
unsafe fn build_slot(s: usize) {
    let kpml4 = kernel_cr3() as *const u64; // capture kernel CR3 (BSP, boot) and share its half
    let pml4 = slot_pml4_ptr(s);
    let user_pml4_i = pml4_index(super::syscall::USER_BASE);
    for i in 0..512 {
        // Share every kernel PML4 entry except the per-process user-window slot, which is private.
        let e = if i == user_pml4_i { 0 } else { unsafe { *kpml4.add(i) } };
        unsafe { *pml4.add(i) = e };
    }
    let pdpt = slot_pdpt_ptr(s);
    let pd = slot_pd_ptr(s);
    let pt = slot_pt_ptr(s);
    // Intermediate entries: PRESENT|WRITABLE|USER (ring-3 reach is the AND of U bits on the path;
    // WRITABLE on a parent does not make leaves writable — the leaf W bit governs that).
    let inter = PTE_PRESENT | PTE_WRITABLE | PTE_USER;
    unsafe {
        *pml4.add(user_pml4_i) = (table_pa(pdpt) & PTE_ADDR) | inter;
        *pdpt.add(pdpt_index(super::syscall::USER_BASE)) = (table_pa(pd) & PTE_ADDR) | inter;
        *pd.add(pd_index(super::syscall::USER_BASE)) = (table_pa(pt) & PTE_ADDR) | inter;
    }
    let backing = slot_backing_ptr(s) as u64;
    for p in 0..U3_WINDOW_PAGES {
        let va = super::syscall::USER_BASE + (p as u64) * PAGE_4K;
        let frame = backing + (p as u64) * PAGE_4K;
        // Page 0 = code: USER, executable (no NX), read-only (no WRITABLE). Pages 1..N = data/stack:
        // USER, WRITABLE, NX.
        let mut flags = PTE_PRESENT | PTE_USER;
        if p != 0 {
            flags |= PTE_WRITABLE | PTE_NX;
        }
        unsafe { *pt.add(pt_index(va)) = (frame & PTE_ADDR) | flags };
    }
}

/// Allocate a fresh per-process address space from the static pool and build its window. Returns the
/// slot index, or `None` when the pool is exhausted (a STOP tripwire — the hard cap is deliberate).
pub fn alloc_user_space() -> Option<usize> {
    for s in 0..USER_SLOTS {
        if SLOT_USED[s]
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            unsafe { build_slot(s) };
            return Some(s);
        }
    }
    None
}

/// Allocate `out.len()` slots, filling `out` with their indices. FULL UNWIND on partial failure
/// (mirrors M6d's `alloc_user_slots`): a slot that was claimed but never installed is released with
/// no TLB work, so exhaustion never leaks a partial claim. Returns false if the pool can't satisfy
/// the whole request.
pub fn alloc_user_spaces(out: &mut [usize]) -> bool {
    let mut n = 0;
    while n < out.len() {
        match alloc_user_space() {
            Some(s) => {
                out[n] = s;
                n += 1;
            }
            None => {
                for &s in &out[..n] {
                    SLOT_USED[s].store(false, Ordering::Release); // never installed -> no TLB flush
                }
                return false;
            }
        }
    }
    true
}

/// Release the slot whose CR3 is `cr3`. The caller MUST have already restored the kernel CR3 (that
/// `mov cr3` full-flush is what retires this slot's user TLB entries) — see `restore_kernel_cr3`.
pub fn free_user_space_by_cr3(cr3: u64) {
    for s in 0..USER_SLOTS {
        if slot_cr3(s) == cr3 {
            // U5x teardown-clear: wipe the slot's per-process handle row (values + rights) BEFORE
            // releasing the used-flag (clear-before-release), so no capability outlives its owning slot
            // and no concurrent `alloc_user_space` can claim the slot and populate the row in between.
            // Both user-task teardown paths funnel through here — normal `exit` and the KillSwitch reap
            // of a never-exiting preemptible ring-3 task — so the clear rides both.
            super::syscall::clear_handle_row(s);
            // WINX-1 teardown-clear, on the same clear-before-release discipline as the handle row:
            // retire this slot's compositor windows FIRST (so nothing is left composing from backing
            // we are about to unmap), then drop the FB leaves and zero the region. Both must precede
            // the used-flag release, or a concurrent `alloc_user_space` could claim the slot and its
            // first `SYS_WIN_CREATE` would land on the previous tenant's live window rows.
            super::syscall::win_close_slot(s);
            unsafe { clear_slot_fb(s) };
            SLOT_USED[s].store(false, Ordering::Release);
            return;
        }
    }
}

/// U3 DETERMINISTIC ISOLATION PROBE (the metal-catchable proof — x86 twin of M6d's nG probe). Plant
/// DISTINCT sentinels at the same user VA in two slots' data pages, then — interrupts masked — install
/// each slot's CR3 in turn and read that same VA, confirming each reads its OWN sentinel. A bug that
/// let the two share PML4[2] (or failed to isolate the window) reads the same value twice. Restores
/// the kernel CR3 before returning. `data_off` is the byte offset within the data page (page 1).
/// Returns `(a_val, b_val, ok)`.
pub fn probe_isolation(
    slot_a: usize,
    slot_b: usize,
    data_off: u64,
    sent_a: u64,
    sent_b: u64,
) -> (u64, u64, bool) {
    let cr3_a = slot_cr3(slot_a);
    let cr3_b = slot_cr3(slot_b);
    let kcr3 = kernel_cr3();
    let page_off = (PAGE_4K + data_off) as usize; // data page = window page 1
    let data_va = super::syscall::USER_BASE + PAGE_4K + data_off;
    // Plant the sentinels through the backing identity aliases (kernel half — always mapped).
    unsafe {
        core::ptr::write_volatile(slot_backing_ptr(slot_a).add(page_off) as *mut u64, sent_a);
        core::ptr::write_volatile(slot_backing_ptr(slot_b).add(page_off) as *mut u64, sent_b);
    }
    // Swap CR3 to each slot in turn and read the SAME user VA at CPL 0 (no SMAP on Ivy Bridge, so a
    // supervisor read of a user page is allowed; SMEP blocks only supervisor EXECUTE of user pages).
    let (a_val, b_val) = crate::arch::without_interrupts(|| unsafe {
        load_cr3(cr3_a);
        let a = core::ptr::read_volatile(data_va as *const u64);
        load_cr3(cr3_b);
        let b = core::ptr::read_volatile(data_va as *const u64);
        load_cr3(kcr3);
        (a, b)
    });
    // Review fold (M6d): distinct-value assert — identical reads mean the windows aliased.
    debug_assert!(a_val != b_val, "U3: isolation probe read identical values — windows not isolated");
    (a_val, b_val, a_val == sent_a && b_val == sent_b)
}

// =================================================================================================
// VPERF-WC — write-combining for the framebuffer mapping (x86, memory-TYPE only; seat-signed scope).
// -------------------------------------------------------------------------------------------------
// The M3 cached-RAM shadow already turned all VRAM traffic into write-only sequential blits
// (`vread=0`, metal-confirmed). The metal round-6 rMBP readout showed the fb still effective-UC
// (var-range MTRR UC, PAT=WB in the l2 PTE) — so those posted writes are NOT coalesced. Marking the
// framebuffer Write-Combining lets the CPU's WC buffers combine the sequential stores (~10x on the
// write path is the expectation the next bench measures).
//
// SCOPE (matches the seat sign-off verbatim): this changes MEMORY TYPE ONLY, and ONLY on the leaves
// that map the framebuffer. It touches NO page-permission bit (PRESENT/WRITABLE/USER/NX untouched),
// NO MTRR, and no other mapping. SMEP/NXE/W^X are unaffected (they live in CR4/EFER and the U/W/NX
// PTE bits, none of which we write). The mechanism is a single unused PAT slot + the fb leaves'
// PAT/PCD/PWT selector bits.
// =================================================================================================

const IA32_PAT_MSR: u32 = 0x277;
/// PAT slot we repurpose to Write-Combining. Power-on PAT is [WB,WT,UC-,UC] in entries 0..3 and
/// DUPLICATES them in 4..7. No firmware/kernel mapping ever sets the PTE PAT bit, so entries 4..7 are
/// unused — we set PA4 = WC (encoding 0x01). A PTE selecting index 4 (PAT=1,PCD=0,PWT=0) then reads
/// WC; leaving 0..3 alone keeps every live mapping's effective type byte-identical.
const PAT_WC_INDEX: u64 = 4;
/// Architectural PAT memory-type encoding for Write-Combining.
const PAT_TYPE_WC: u64 = 0x01;

/// Program THIS CPU's IA32_PAT so slot `PAT_WC_INDEX` == WC, preserving every other slot. Idempotent
/// and harmless on any CPU: no live mapping selects that slot until a PTE opts in via the PAT bit.
/// The BSP is programmed by `set_framebuffer_wc` (it drives the early console/scenario — the
/// originally measured path). EVERY AP is programmed by `smp::ap_entry`, which calls this one line
/// right after `apic::init()` and before the `AP_ONLINE` handshake — so every core that can ever
/// reach the scheduler loop has PA4=WC before it is released.
///
/// SCHED-X86 made that wiring load-bearing rather than cosmetic. The render service is now a
/// scheduled kernel task pinned to an AP, so an AP is THE blitting core. Were its PAT left at the
/// power-on default, its fb PTE would select PA4=WB which, under the firmware's UC var-range MTRR,
/// is EFFECTIVE-UC: the sequential blit stores would stop being write-combined. (That state was
/// never a correctness bug — WC and UC are both uncacheable, so it is not the SDM 11.12.4
/// WB-aliasing hazard, just the same write-only access pattern un-accelerated — but on a panel whose
/// flush is already most of a frame it is the difference between a working desktop and a hang.)
/// Programming every core, not only the one currently chosen, is deliberate: it makes the placement
/// decision non-load-bearing, so re-pinning render later cannot silently regress the panel.
pub fn ensure_pat_wc() {
    use x86_64::registers::model_specific::Msr;
    // PAT support: CPUID.01H:EDX[16]. A part without PAT never gets the WRMSR.
    let has_pat = core::arch::x86_64::__cpuid(1).edx & (1 << 16) != 0;
    if !has_pat {
        return;
    }
    unsafe {
        let mut pat = Msr::new(IA32_PAT_MSR);
        let cur = pat.read();
        let shift = 8 * PAT_WC_INDEX;
        let want = (cur & !(0xFFu64 << shift)) | (PAT_TYPE_WC << shift);
        if want != cur {
            pat.write(want);
        }
    }
}

/// Runs the fb-leaf retype exactly once (the PAT program is idempotent and re-runs harmlessly).
static FB_WC_DONE: AtomicBool = AtomicBool::new(false);

/// Physical span of the LEAVES `set_framebuffer_wc` typed WC, as `[FB_WC_LO, FB_WC_HI)`. Empty
/// while `LO >= HI` (the initial state), which is the honest encoding of "nothing retyped yet".
///
/// This exists so `map_mmio_window` can tell a deliberately-typed leaf from an ordinary one. It is
/// the retyped LEAVES' span, not the caller's `[fb_base, fb_base+fb_len)`: the leaves are huge, so
/// the protected extent is rounded OUT to leaf boundaries — that is exactly the extent whose memory
/// type an MMIO remap would actually change, so the range and the hazard have the same granularity.
/// Recorded as a single min/max interval rather than a list: the fb is one contiguous aperture, and
/// the containment test is only half of the guard (see `leaf_is_fb_wc`), so a conservative hull
/// costs nothing in precision.
static FB_WC_LO: AtomicU64 = AtomicU64::new(u64::MAX);
static FB_WC_HI: AtomicU64 = AtomicU64::new(0);

/// True iff the leaf `[leaf_start, leaf_start+leaf_size)` is one `set_framebuffer_wc` typed WC and
/// which STILL carries that type. Deliberately a conjunction of two independent facts:
///   1. it overlaps the recorded WC leaf span — i.e. `set_framebuffer_wc` really walked over it; and
///   2. its PAT bit is still set — i.e. it really is still selecting PA4 (=WC) and not something a
///      later edit already changed.
///
/// Either half alone would be too loose. (1) alone would protect an unmapped/never-retyped leaf that
/// merely falls inside the hull; (2) alone would infer intent from a bit that, while this tree never
/// sets it anywhere else (see `PAT_WC_INDEX`), is ultimately firmware-controlled. Requiring both
/// means the only leaves that escape UC typing are ones we can point at a `:: x86 fb-wc:` line for.
fn leaf_is_fb_wc(entry: u64, leaf_start: u64, leaf_size: u64, pat_bit: u64) -> bool {
    if entry & pat_bit == 0 {
        return false;
    }
    let (lo, hi) = (FB_WC_LO.load(Ordering::Acquire), FB_WC_HI.load(Ordering::Acquire));
    if lo >= hi {
        return false; // no fb retype has happened — nothing to protect
    }
    let leaf_end = leaf_start.saturating_add(leaf_size);
    leaf_start < hi && lo < leaf_end
}

/// Mutable pointer to the LEAF entry mapping `va` + its level (1 = 4 KiB, 2 = 2 MiB, 3 = 1 GiB), or
/// `None` if unmapped. The write-capable twin of `translate` — we need the entry's address to retype
/// its memory-type selector bits.
fn leaf_entry_ptr(va: u64) -> Option<(*mut u64, u8)> {
    unsafe {
        let pml4e_p = cr3_table().add(pml4_index(va));
        if *pml4e_p & PTE_PRESENT == 0 {
            return None;
        }
        let pdpte_p = ((*pml4e_p & PTE_ADDR) as *mut u64).add(pdpt_index(va));
        if *pdpte_p & PTE_PRESENT == 0 {
            return None;
        }
        if *pdpte_p & PTE_HUGE != 0 {
            return Some((pdpte_p, 3));
        }
        let pde_p = ((*pdpte_p & PTE_ADDR) as *mut u64).add(pd_index(va));
        if *pde_p & PTE_PRESENT == 0 {
            return None;
        }
        if *pde_p & PTE_HUGE != 0 {
            return Some((pde_p, 2));
        }
        let pte_p = ((*pde_p & PTE_ADDR) as *mut u64).add(pt_index(va));
        if *pte_p & PTE_PRESENT == 0 {
            return None;
        }
        Some((pte_p, 1))
    }
}

/// Invalidate the TLB entry covering `va` (works for huge and GLOBAL entries, unlike a CR3 reload —
/// firmware huge identity leaves may carry the Global bit).
#[inline]
unsafe fn invlpg(va: u64) {
    unsafe { core::arch::asm!("invlpg [{}]", in(reg) va, options(nostack, preserves_flags)) };
}

/// Mark the framebuffer mapping Write-Combining: program PA4=WC on the BSP, then retype every
/// identity-map leaf that covers `[fb_base, fb_base+fb_len)` to select that slot (set the PAT bit,
/// clear PCD/PWT), and flush those TLB entries. Runs once (the retype is guarded; the PAT program is
/// idempotent).
///
/// Granularity honesty: the firmware maps the fb with 2 MiB (metal + QEMU both show `l2`) — sometimes
/// 1 GiB — leaves, so the retyped span is the SET OF HUGE-PAGE LEAVES that contain the range. Every
/// such leaf lies inside the GPU's own BAR aperture (device MMIO), never RAM/heap/kernel — so "the
/// framebuffer's mapping" is exactly what changes; no unrelated mapping is touched. `wbinvd` is NOT
/// issued: the fb was effective-UC before this (metal-confirmed + firmware default), so no cache line
/// holds fb data that would need writing back.
pub fn set_framebuffer_wc(fb_base: u64, fb_len: u64) {
    if fb_base == 0 || fb_len == 0 {
        return;
    }
    // Program the WC PAT slot on this (BSP) CPU first, so the fb reads WC the instant it is retyped.
    ensure_pat_wc();
    if FB_WC_DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    // BPACE HPACE-1: the `heap` block's first sub-stamp. It sits AFTER the one-shot latch so it
    // records exactly once, on the pass that actually retypes. See bootpace.md §11.
    crate::bootpace::record("fb-wc");
    let fb_end = fb_base.saturating_add(fb_len);
    let mut leaves = 0u32;
    // Retype the fb leaves. Firmware page-table pages are read-only under CR0.WP=1, so drop WP for
    // the window (IRQs held off; WP restored on exit) — the exact page-table-edit sequence the U1a
    // mapper uses. We write ONLY the PAT/PCD/PWT selector bits; every permission bit is preserved.
    with_page_tables_writable(|| {
        let mut va = fb_base;
        while va < fb_end {
            match leaf_entry_ptr(va) {
                Some((entry, level)) => {
                    let leaf_size: u64 = match level {
                        3 => 1 << 30,
                        2 => 1 << 21,
                        _ => 1 << 12,
                    };
                    // The PAT bit sits at bit 7 in a 4 KiB PTE but bit 12 in a 2 MiB/1 GiB leaf.
                    let pat_bit = pat_bit_for_level(level);
                    let leaf_start = va & !(leaf_size - 1);
                    unsafe {
                        let e = *entry;
                        // Select PAT index 4 = (PAT=1, PCD=0, PWT=0). Memory type only.
                        let ne = (e | pat_bit) & !(PTE_PCD | PTE_PWT);
                        if ne != e {
                            *entry = ne;
                            leaves += 1;
                        }
                        // Publish the leaf's extent so `map_mmio_window` will not later retype it
                        // back to UC. Recorded for every leaf that ENDS UP WC, not only the ones
                        // this call changed: a leaf that was already WC is just as deliberate, and
                        // just as easy for a BAR remap that contains it to silently clobber.
                        if *entry & pat_bit != 0 {
                            FB_WC_LO.fetch_min(leaf_start, Ordering::AcqRel);
                            FB_WC_HI.fetch_max(leaf_start.saturating_add(leaf_size), Ordering::AcqRel);
                        }
                    }
                    va = leaf_start.saturating_add(leaf_size);
                }
                None => va = va.saturating_add(1 << 12), // unmapped gap — step one page
            }
        }
    });
    // Flush the retyped span from the TLB. Step 4 KiB so any leaf granularity (and any Global huge
    // entry) is covered; one-time boot cost, invlpg is a couple of cycles each.
    let mut va = fb_base & !((1u64 << 12) - 1);
    while va < fb_end {
        unsafe { invlpg(va) };
        va = va.saturating_add(1 << 12);
    }
    serial_println!(
        ":: x86 fb-wc: retyped {} leaf(s) WC (PAT PA4) over {:#x}..{:#x} ::",
        leaves,
        fb_base,
        fb_end
    );
    // BPACE HPACE-1: `fb-wc-done d=` is the retype itself — the leaf walk, the 4 KiB `invlpg` sweep
    // over the whole span, and this one line. Everything the retype is BLAMED for costs exactly this
    // much, and the number is now on the wire instead of being assumed small.
    crate::bootpace::record("fb-wc-done");
}

/// Map a physical MMIO window into the identity map with Uncacheable (UC) attributes, creating
/// intermediate page tables where they are absent.
///
/// ## What "UC" means here, precisely
/// A leaf's memory type is an INDEX into IA32_PAT, assembled from three PTE bits as (PAT, PCD, PWT).
/// This function writes index 3 = (PAT=0, PCD=1, PWT=1). Power-on PAT is [WB, WT, UC-, UC] in
/// entries 0..3 and duplicates them in 4..7 (see `PAT_WC_INDEX`), and the only slot this kernel ever
/// reprograms is PA4 — `ensure_pat_wc` writes that one slot and preserves all seven others, on the
/// BSP and on every AP. So PA3 is UC (strong, uncacheable, never combined, never reordered by the
/// memory type) on every core for the life of the boot. Device register windows get exactly that.
///
/// This is a behavioural change from the previous `*entry |= PCD|PWT`, which left the PAT bit alone.
/// On a leaf with PAT already clear the two are identical (index 3 either way). They differ only on a
/// leaf that carries PAT=1, where the old code produced index 7 — also UC by the power-on table, so
/// the old code was not wrong about device windows; it was wrong about the ONE leaf class that opts
/// out of the table, below.
///
/// ## The Write-Combining exception
/// `set_framebuffer_wc` deliberately retypes the framebuffer's leaves to PA4 = WC and latches itself
/// so it never runs again. The Kepler BAR1 aperture (256 MiB at the fb's own base) CONTAINS those
/// leaves, so mapping BAR1 as an MMIO window used to silently drag the panel back to uncacheable and
/// nothing ever put it back: metal measured a flat ~162 MB/s, size-invariant, against 7.6 -> 53.8 fps
/// with WC live. A window mapping is a statement about a range that the caller has NOT looked at leaf
/// by leaf; it must not overrule a per-leaf decision someone else made on purpose. So a leaf that
/// `leaf_is_fb_wc` recognises is SKIPPED — not re-typed, not partially re-typed, not touched at all.
///
/// Merely clearing the PAT bit alongside PCD|PWT (index 7 -> index 3) would have made this function's
/// old doc comment true and fixed nothing: both indices are UC, so the panel would still be slow.
/// The skip is the part that matters; the PAT clear is for the leaves we DO type, so that "UC" here
/// names one specific PAT slot instead of two that merely happen to agree today.
///
/// Genuine MMIO stays UC by construction. `leaf_is_fb_wc` demands BOTH that the leaf overlap the span
/// `set_framebuffer_wc` actually walked AND that its PAT bit still be set. A BAR0 register window, a
/// second device, any leaf no fb retype ever covered — none can satisfy the first condition, so every
/// one of them takes the UC path exactly as before. Before any fb retype has run at all the span is
/// empty and the predicate is constant-false, so early-boot windows (SDHC, bench) are unaffected.
///
/// The guard is therefore order-dependent, and that is fine in both directions. `fbcon::init` calls
/// `set_framebuffer_wc` during console bring-up, long before any GPU probe, which is the order that
/// needs the guard. The reverse order needs nothing: a window mapped BEFORE the fb retype is typed UC
/// here and then retyped WC by `set_framebuffer_wc` afterwards, which is the correct end state anyway.
///
/// Granularity honesty: the identity map's leaves are 2 MiB (metal + QEMU both show `l2`) or 1 GiB, so
/// typing a window types whole leaves — a small BAR makes its entire containing leaf UC. That was
/// already true and is unchanged. The consequence for the skip is the mirror image: if the fb shares a
/// leaf with a device register, that register was ALREADY WC the moment `set_framebuffer_wc` ran, and
/// skipping preserves that state rather than creating it. This function cannot separate them without
/// splitting the leaf, and it does not split leaves.
///
/// Permissions are never touched: every write below is a read-modify-write of the PAT/PCD/PWT
/// selector bits only. P/RW/U/S/NX/G/A/D and the address field are carried through unchanged. The only
/// entries whose permission bits this function sets are ones it CREATES (absent intermediate tables,
/// and absent 4 KiB leaves, which it makes PRESENT|WRITABLE|NX as before).
///
/// `wbinvd` is not issued, for the same reason `set_framebuffer_wc` does not: these are device
/// apertures the CPU has not cached (firmware types them UC via var-range MTRRs), so no line holds
/// data that a type change could strand.
pub fn map_mmio_window(pa: u64, size: usize) {
    if pa == 0 || size == 0 {
        return;
    }
    let end = pa.saturating_add(size as u64);
    // Leaves we asserted UC, and leaves left alone because they are the fb's deliberate WC typing.
    // Counted per LEAF, so a window that walks 4 KiB entries counts pages — the sum is not a fixed
    // function of the window size, which is the point: it says what the walk actually found.
    let mut uc_leaves = 0u32;
    let mut wc_kept = 0u32;

    with_page_tables_writable(|| {
        let mut va = pa;
        while va < end {
            unsafe {
                let pml4e = cr3_table().add(pml4_index(va));
                if *pml4e & PTE_PRESENT == 0 {
                    let frame = alloc_page_frame();
                    *pml4e = (frame & PTE_ADDR) | PTE_PRESENT | PTE_WRITABLE;
                }

                let pdpt = (*pml4e & PTE_ADDR) as *mut u64;
                let pdpte = pdpt.add(pdpt_index(va));
                if *pdpte & PTE_PRESENT != 0 && *pdpte & PTE_HUGE != 0 {
                    let leaf_start = va & !((1u64 << 30) - 1);
                    if leaf_is_fb_wc(*pdpte, leaf_start, 1 << 30, PTE_PAT_HUGE) {
                        wc_kept += 1;
                    } else {
                        *pdpte = (*pdpte | PTE_PCD | PTE_PWT) & !PTE_PAT_HUGE;
                        uc_leaves += 1;
                    }
                    va = leaf_start.saturating_add(1 << 30);
                    continue;
                }
                if *pdpte & PTE_PRESENT == 0 {
                    let frame = alloc_page_frame();
                    *pdpte = (frame & PTE_ADDR) | PTE_PRESENT | PTE_WRITABLE;
                }

                let pd = (*pdpte & PTE_ADDR) as *mut u64;
                let pde = pd.add(pd_index(va));
                if *pde & PTE_PRESENT != 0 && *pde & PTE_HUGE != 0 {
                    let leaf_start = va & !((1u64 << 21) - 1);
                    if leaf_is_fb_wc(*pde, leaf_start, 1 << 21, PTE_PAT_HUGE) {
                        wc_kept += 1;
                    } else {
                        *pde = (*pde | PTE_PCD | PTE_PWT) & !PTE_PAT_HUGE;
                        uc_leaves += 1;
                    }
                    va = leaf_start.saturating_add(1 << 21);
                    continue;
                }
                if *pde & PTE_PRESENT == 0 {
                    let frame = alloc_page_frame();
                    *pde = (frame & PTE_ADDR) | PTE_PRESENT | PTE_WRITABLE;
                }

                let pt = (*pde & PTE_ADDR) as *mut u64;
                let pte = pt.add(pt_index(va));
                if *pte & PTE_PRESENT == 0 {
                    // Fresh leaf: PAT is clear by construction, so this is index 3 = UC.
                    *pte = (va & PTE_ADDR) | PTE_PRESENT | PTE_WRITABLE | PTE_NX | PTE_PCD | PTE_PWT;
                    uc_leaves += 1;
                } else if leaf_is_fb_wc(*pte, va & !((1u64 << 12) - 1), 1 << 12, PTE_PAT_4K) {
                    // The 4 KiB path carries the identical hazard: `set_framebuffer_wc` retypes
                    // whatever leaf level it finds, and a firmware that mapped the fb with 4 KiB PTEs
                    // would leave PAT at bit 7 here. Same rule, different bit.
                    wc_kept += 1;
                } else {
                    *pte = (*pte | PTE_PCD | PTE_PWT) & !PTE_PAT_4K;
                    uc_leaves += 1;
                }

                va = va.saturating_add(1 << 12);
            }
        }
    });

    let mut flush_va = pa & !((1u64 << 12) - 1);
    while flush_va < end {
        unsafe { invlpg(flush_va) };
        flush_va = flush_va.saturating_add(1 << 12);
    }

    // Wire line: says what the walk DID, so a boot log can show the WC leaves surviving a containing
    // BAR map rather than leaving it to be inferred from a frame rate. `wc-kept` is nonzero only for
    // a window that overlaps the framebuffer's leaves; every other window prints `wc-kept=0`.
    serial_println!(
        ":: x86 mmio-map: {:#x}..{:#x} uc={} (PAT PA3) wc-kept={} ::",
        pa,
        end,
        uc_leaves,
        wc_kept
    );
}
