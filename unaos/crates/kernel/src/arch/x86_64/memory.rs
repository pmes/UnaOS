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
const PAGE_4K: u64 = 0x1000;

/// A 4 KiB-aligned page-table (512 × u64). In `.bss` (identity-mapped), so its address IS its
/// physical address — usable directly as a CR3 / next-level pointer.
#[repr(C, align(4096))]
struct PageTable([u64; 512]);
impl PageTable {
    const fn zeroed() -> Self {
        PageTable([0; 512])
    }
}

/// A slot's user backing store: `U3_WINDOW_PAGES` contiguous 4 KiB frames (code + data + 2 stack).
#[repr(C, align(4096))]
struct Backing([u8; U3_WINDOW_PAGES * 4096]);
impl Backing {
    const fn zeroed() -> Self {
        Backing([0; U3_WINDOW_PAGES * 4096])
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
/// The BSP is programmed by `set_framebuffer_wc` (it drives the console/scenario — the measured
/// path). For a uniform effective type on APs that also blit, an AP would call this at bring-up
/// (a one-line `arch::memory::ensure_pat_wc()` in `smp::ap_entry`); left as a follow-up so this arc
/// stays in-lane. Not wiring it is CORRECT, not merely tolerated: an AP's fb PTE then selects PA4=WB
/// (its unmodified default) which, under the UC var-range MTRR, is effective-UC — so an AP blit is
/// plain UC (no speedup) while the BSP blit is WC. WC and UC are both uncacheable (no cache line
/// holds fb data), so the mix is not the SDM 11.12.4 WB-aliasing hazard — it is exactly the
/// write-only-framebuffer access pattern, just un-accelerated on the AP.
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
                    let pat_bit: u64 = if level == 1 { 1 << 7 } else { 1 << 12 };
                    let pcd: u64 = 1 << 4;
                    let pwt: u64 = 1 << 3;
                    unsafe {
                        let e = *entry;
                        // Select PAT index 4 = (PAT=1, PCD=0, PWT=0). Memory type only.
                        let ne = (e | pat_bit) & !(pcd | pwt);
                        if ne != e {
                            *entry = ne;
                            leaves += 1;
                        }
                    }
                    va = (va & !(leaf_size - 1)).saturating_add(leaf_size);
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
}

/// Map a physical MMIO window into the identity map with Uncacheable (UC) attributes (PCD|PWT).
/// Creates intermediate page tables if they are absent.
/// If an existing huge page leaf is encountered, it applies PCD|PWT without clearing the HUGE/PAT bit.
pub fn map_mmio_window(pa: u64, size: usize) {
    if pa == 0 || size == 0 {
        return;
    }
    let end = pa.saturating_add(size as u64);
    let pcd = 1 << 4;
    let pwt = 1 << 3;

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
                    *pdpte |= pcd | pwt;
                    va = (va & !((1 << 30) - 1)).saturating_add(1 << 30);
                    continue;
                }
                if *pdpte & PTE_PRESENT == 0 {
                    let frame = alloc_page_frame();
                    *pdpte = (frame & PTE_ADDR) | PTE_PRESENT | PTE_WRITABLE;
                }
                
                let pd = (*pdpte & PTE_ADDR) as *mut u64;
                let pde = pd.add(pd_index(va));
                if *pde & PTE_PRESENT != 0 && *pde & PTE_HUGE != 0 {
                    *pde |= pcd | pwt;
                    va = (va & !((1 << 21) - 1)).saturating_add(1 << 21);
                    continue;
                }
                if *pde & PTE_PRESENT == 0 {
                    let frame = alloc_page_frame();
                    *pde = (frame & PTE_ADDR) | PTE_PRESENT | PTE_WRITABLE;
                }
                
                let pt = (*pde & PTE_ADDR) as *mut u64;
                let pte = pt.add(pt_index(va));
                if *pte & PTE_PRESENT == 0 {
                    *pte = (va & PTE_ADDR) | PTE_PRESENT | PTE_WRITABLE | PTE_NX | pcd | pwt;
                } else {
                    *pte |= pcd | pwt;
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
}
