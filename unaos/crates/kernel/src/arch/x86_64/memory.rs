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
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
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

/// Walk the live CR3 tables for `va` and return BOTH the physical address it maps to AND the FOLDED
/// `(user, writable, nx)` permission triple the hardware itself computes for it — or `None` if any
/// level on the path is not present. Honors 1 GiB / 2 MiB huge pages. Read-only.
///
/// THE FOLD IS THE POINT. A leaf's effective permission on x86-64 is not its own entry, it is the
/// accumulation over PML4→PDPT→PD→PT: ring-3 reachability is the AND of U/S, writability is the AND
/// of W, executability is NOT the OR of NX. That accumulation is `wx_fold` — the SAME pure function
/// the WXAUDIT census, `wx_check_slot` and the WXAUDIT-0 negative control are built on, reused here
/// rather than re-derived, so there is exactly one permission model in this file and one place a
/// mistake in it can live. `translate` is the address half of this walk and nothing else.
fn live_leaf(va: u64) -> Option<(u64, (bool, bool, bool))> {
    unsafe {
        let pml4e = *cr3_table().add(pml4_index(va));
        if pml4e & PTE_PRESENT == 0 {
            return None;
        }
        let acc4 = wx_fold(WX_TOP, pml4e);
        let pdpte = *((pml4e & PTE_ADDR) as *const u64).add(pdpt_index(va));
        if pdpte & PTE_PRESENT == 0 {
            return None;
        }
        let acc3 = wx_fold(acc4, pdpte);
        if pdpte & PTE_HUGE != 0 {
            return Some(((pdpte & PTE_ADDR) | (va & 0x3FFF_FFFF), acc3));
        }
        let pde = *((pdpte & PTE_ADDR) as *const u64).add(pd_index(va));
        if pde & PTE_PRESENT == 0 {
            return None;
        }
        let acc2 = wx_fold(acc3, pde);
        if pde & PTE_HUGE != 0 {
            return Some(((pde & PTE_ADDR) | (va & 0x1F_FFFF), acc2));
        }
        let pte = *((pde & PTE_ADDR) as *const u64).add(pt_index(va));
        if pte & PTE_PRESENT == 0 {
            return None;
        }
        Some(((pte & PTE_ADDR) | (va & 0xFFF), wx_fold(acc2, pte)))
    }
}

/// Walk the live CR3 tables and return the physical address `va` maps to, or `None` if any level
/// on the path is not present. Honors 1 GiB / 2 MiB huge pages. Read-only — used to prove the user
/// window is unmapped before we create it (so USER_BASE can never silently alias kernel memory).
pub fn translate(va: u64) -> Option<u64> {
    live_leaf(va).map(|(pa, _)| pa)
}

/// **CFU-2 — the authoritative kernel/user access gate.** True iff EVERY 4 KiB page `[va, va + len)`
/// touches is, in the LIVE CR3, a present leaf that ring 3 can reach (`user`) and — when `need_write`
/// — one the CPU will let a store land on (`writable`, folded over the whole path, which is exactly
/// what CR0.WP=1 enforces for a CPL-0 store).
///
/// WHY THIS EXISTS. `syscall::user_range_ok` used to decide "is this a legal kernel→user write
/// destination?" from BOUNDS ALONE, excluding exactly one page (`USER_BASE + PAGE_SIZE`). That
/// encodes the FLAT/U2 window layout, where page 0 is the only code page. The U3 ELF loader does not
/// obey it: `protect_user_slot_range` clears `PTE_WRITABLE` on EVERY page a non-`PF_W` segment
/// covers, so a program whose first `R E` LOAD is larger than one page (`VUG-X86.ELF`, memsz 0x1fe5;
/// `PULSE-X86.ELF`, memsz 0x13ec — both already on the boot media) has page 1 read-only AND
/// executable. `USER_BASE + 0x1000` was the FIRST address the old bound admitted, so a ring-3
/// `sys_read`/`sys_recvfrom` aimed there made the kernel itself store into a ring-3 RX page: code
/// injection with the kernel as the writer (silent while CR0.WP is clear; a fatal CPL-0 #PF once it
/// is armed). Bounds cannot answer that question — only the live leaf can, so the leaf decides.
///
/// PER PAGE, ACROSS THE WHOLE RANGE. A range may straddle a writable leaf and a non-writable one
/// (page 2 RW → page 3 RO is a shape `protect_user_slot_range` can produce), so a first-page check
/// would still admit the tail. Every page in the range is walked. `len == 0` validates the page
/// containing `va`, matching the historical "a zero-length range is legal iff its pointer is" rule.
///
/// Cost is bounded by the caller's own bounds pre-filter: a validated range never exceeds the 4-page
/// ring-3 window, so this is at most five 4-level walks (≈20 dependent loads) per syscall.
pub fn user_range_leaf_ok(va: u64, len: u64, need_write: bool) -> bool {
    // Overflow-safe last byte. `len == 0` -> the page holding `va` itself.
    let last = if len == 0 {
        va
    } else {
        match va.checked_add(len - 1) {
            Some(v) => v,
            None => return false, // wraps past u64::MAX — the bounds pre-filter rejects this too
        }
    };
    let stop = last & !(PAGE_4K - 1);
    let mut p = va & !(PAGE_4K - 1);
    loop {
        match live_leaf(p) {
            // `acc.0` = ring-3 reachable (U/S folded), `acc.1` = writable (W folded). NX is not
            // consulted: this gate governs data access, not execution.
            Some((_, acc)) => {
                if !acc.0 || (need_write && !acc.1) {
                    return false;
                }
            }
            None => return false, // not present anywhere on the path
        }
        if p == stop {
            return true;
        }
        p += PAGE_4K;
    }
}

/// The LIVE CR3 base as a raw value. Used by the CFU-2 self-probe to save and restore the address
/// space around a deliberate switch into a scratch slot; `kernel_cr3()` is NOT a substitute (it
/// returns the captured kernel PML4, which is not necessarily what is installed right now).
pub fn current_cr3() -> u64 {
    cr3_table() as u64
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
    // WXAUDIT: the W^X gate on the ONE ring-3 mapping seam that lacked it. `protect_user_slot_range`
    // already refuses a W+X segment and `map_slot_fb_page`/`build_slot` mint constant shapes, but this
    // helper takes `writable` and `nx` as INDEPENDENT arguments, so nothing stopped a future caller from
    // asking for a ring-3 page that is both writable and executable. Fail-closed at the seam, matching
    // `protect_user_slot_range`'s assert — a W+X user page is a kernel bug, never bad input.
    assert!(
        !(writable && !nx),
        "WXAUDIT: W^X — a ring-3 page may not be both writable and executable"
    );
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
            // WXAUDIT: verify the window we just built against the LIVE table bytes before the slot can
            // be dispatched. `build_slot` writes constants that are correct today; this checks what is
            // actually in the PT, so a future edit that makes a user page W+X fails here — at the
            // allocation that would have handed it to ring 3 — instead of silently shipping.
            wx_check_slot(s);
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
// WXAUDIT — the x86_64 W^X map audit. The twin of the aarch64 H1/M6c audit table in
// `docs/SECURITY.md`, which x86 has never had: the x86 ledger's "W^X audit of kernel mappings (no
// page both writable and executable)" box has been open since U1a.
// -------------------------------------------------------------------------------------------------
// WHAT IT MEASURES, AND WHY LEAF BITS ALONE WOULD BE WRONG. On x86-64 a page's effective permission
// is not its leaf entry — it is the FOLD of every entry on the PML4→PDPT→PD→PT path:
//   * ring-3 reachable  = AND of U/S over the path (one supervisor parent hides the whole subtree);
//   * writable          = AND of W over the path;
//   * executable        = NOT (OR of NX over the path)  — NX anywhere above vetoes execution.
// A checker that read only the leaf would BOTH miss violations (a W leaf under a non-U parent is not
// really user-reachable — a false positive) and manufacture them. `wx_fold` is that accumulation,
// and it is a pure function precisely so the negative control below can drive it.
//
// THE VACUITY LEG (the reason this is not a decorative counter). Every NX conclusion above is void
// unless EFER.NXE is actually set — with NXE=0 the hardware IGNORES bit 63 and every present page is
// executable, so a report of "0 W^X violations" computed from NX bits would be a lie. The audit
// therefore READS EFER.NXE and, when it is clear, classifies with NX disregarded (every writable page
// counts as W∧X) and says so on the line. The audit cannot report a clean map on a kernel whose NX is
// not armed.
//
// SCOPE. Read-only: it walks the live tables and writes nothing. It is not WXN/`CR0.WP` enforcement —
// the kernel's own coarse RWX identity RAM (the aarch64 H1 finding, and, as the boot line now shows,
// the x86 finding too) is REPORTED, not fixed; splitting the kernel identity map into RO-executable
// text and NX data is a separate, review-gated arc. What IS enforced here is the ring-3 half, which is
// where the security boundary actually lives: `wx_check_slot` refuses to hand out an address space
// whose user window contains a W∧X page, and `map_user_page`'s gate refuses to mint one.
// =================================================================================================

/// Outcome of a W^X walk. All counts are of PRESENT leaves (4 KiB / 2 MiB / 1 GiB), classified by the
/// FOLDED path permission, never by the leaf entry alone.
#[derive(Clone, Copy, Default)]
pub struct WxAudit {
    /// Present leaves visited.
    pub leaves: u32,
    /// Leaves reachable from ring 3 (U/S set at every level).
    pub user_leaves: u32,
    /// Ring-3-reachable leaves that are BOTH writable and executable. **Must be 0** — this is the
    /// invariant the ledger's "user code page W^X enforced across cores" claim rests on.
    pub user_wx: u32,
    /// Supervisor-only leaves that are both writable and executable (the kernel's coarse identity RAM).
    /// Reported, not asserted: the kernel executes from it, so this is the H1 refactor's debt, not a bug.
    pub kern_wx: u32,
    /// Bytes covered by `kern_wx` leaves.
    pub kern_wx_bytes: u64,
    /// Page-table pages read during the walk (the walk's cost, made visible rather than assumed).
    pub tables: u32,
    /// **O1 — the leaf-level histogram.** Present leaves by the level they terminate at: 1 GiB, 2 MiB,
    /// 4 KiB. `l1g + l2m + l4k == leaves` by construction (every `record` call increments exactly one
    /// of them and `leaves`), and `wx_audit_report` asserts it — a live structural check, not a
    /// decorative one: it is what catches a future `record` call site that forgets its level.
    ///
    /// WHY IT EXISTS. The aggregate `leaves`/`tables`/byte total does **not** determine the leaf mix.
    /// The metal map (`leaves=66047`, `131072 MiB`, `tables=1028`) is satisfied by the whole family
    /// `n1 = k, n4 = 512(k+1), n2 = 65536 − 512k − (k+1)` for k = 0, 1, 2, … — every member matches all
    /// three printed numbers exactly. Only a direct count of the levels can say which one is real, and
    /// the answer decides whether the M2 splitter needs a 1 GiB→2 MiB demotion (and therefore a PD in
    /// its static pool) or only 2 MiB→4 KiB.
    pub l1g: u32,
    pub l2m: u32,
    pub l4k: u32,
    /// True if the table budget was exhausted — the counts are then a LOWER BOUND and the audit
    /// must not be read as a clean bill of health.
    pub truncated: bool,
    /// EFER.NXE as read at audit time. When false, NX was disregarded in the classification above.
    pub nxe: bool,
}

/// Fold one page-table entry into the running `(user, writable, nx)` effective triple.
/// Pure — the negative control drives this directly.
#[inline]
fn wx_fold(acc: (bool, bool, bool), e: u64) -> (bool, bool, bool) {
    (
        acc.0 && (e & PTE_USER != 0),
        acc.1 && (e & PTE_WRITABLE != 0),
        acc.2 || (e & PTE_NX != 0),
    )
}

/// True iff a folded triple describes a page that is simultaneously writable and executable.
/// `nx_honoured` is EFER.NXE: with NX disabled the hardware ignores bit 63, so EVERY writable page is
/// executable and the NX term drops out. Pure — the negative control drives this directly.
#[inline]
fn wx_violates(acc: (bool, bool, bool), nx_honoured: bool) -> bool {
    acc.1 && !(nx_honoured && acc.2)
}

/// The identity accumulator a walk starts from: reachable, writable, not-NX — every restriction must
/// be earned from an actual entry on the path.
const WX_TOP: (bool, bool, bool) = (true, true, false);

/// Read EFER.NXE (bit 11 of IA32_EFER). The audit's honesty hinges on this.
fn efer_nxe() -> bool {
    const IA32_EFER: u32 = 0xC000_0080;
    const EFER_NXE: u64 = 1 << 11;
    // SAFETY: reading IA32_EFER is a pure ring-0 MSR read with no side effects.
    unsafe { x86_64::registers::model_specific::Msr::new(IA32_EFER).read() & EFER_NXE != 0 }
}

impl WxAudit {
    /// Classify one present leaf of `size` bytes under the folded permission `acc`.
    fn record(&mut self, acc: (bool, bool, bool), size: u64, nxe: bool) {
        self.leaves += 1;
        // O1: the level histogram. `size` is the only thing that distinguishes the three call sites,
        // and it is a literal at each of them, so this cannot drift out of step with the walk.
        match size {
            s if s == 1 << 30 => self.l1g += 1,
            s if s == 1 << 21 => self.l2m += 1,
            _ => self.l4k += 1,
        }
        if acc.0 {
            self.user_leaves += 1;
        }
        if wx_violates(acc, nxe) {
            if acc.0 {
                self.user_wx += 1;
            } else {
                self.kern_wx += 1;
                self.kern_wx_bytes += size;
            }
        }
    }
}

/// Walk the 4-level hierarchy rooted at physical `root` (identity-mapped, so it is directly
/// addressable) and classify every present leaf. Honors 1 GiB / 2 MiB huge leaves. Read-only.
///
/// `TABLE_BUDGET` bounds the walk so a pathological map cannot stall boot; exhausting it sets
/// `truncated`, which the report prints — a truncated audit is explicitly NOT a pass.
pub fn wx_audit_root(root: u64) -> WxAudit {
    const TABLE_BUDGET: u32 = 4096;
    let nxe = efer_nxe();
    let mut a = WxAudit { nxe, ..Default::default() };
    let pml4 = root as *const u64;
    a.tables = 1;
    'walk: for i in 0..512 {
        // SAFETY: every page-table frame is identity-mapped, so `root` and each child physical
        // address below are directly readable at ring 0. Reads only.
        let e4 = unsafe { *pml4.add(i) };
        if e4 & PTE_PRESENT == 0 {
            continue;
        }
        let acc4 = wx_fold(WX_TOP, e4);
        if a.tables >= TABLE_BUDGET {
            a.truncated = true;
            break 'walk;
        }
        a.tables += 1;
        let pdpt = (e4 & PTE_ADDR) as *const u64;
        for j in 0..512 {
            let e3 = unsafe { *pdpt.add(j) };
            if e3 & PTE_PRESENT == 0 {
                continue;
            }
            let acc3 = wx_fold(acc4, e3);
            if e3 & PTE_HUGE != 0 {
                a.record(acc3, 1 << 30, nxe); // 1 GiB leaf
                continue;
            }
            if a.tables >= TABLE_BUDGET {
                a.truncated = true;
                break 'walk;
            }
            a.tables += 1;
            let pd = (e3 & PTE_ADDR) as *const u64;
            for k in 0..512 {
                let e2 = unsafe { *pd.add(k) };
                if e2 & PTE_PRESENT == 0 {
                    continue;
                }
                let acc2 = wx_fold(acc3, e2);
                if e2 & PTE_HUGE != 0 {
                    a.record(acc2, 1 << 21, nxe); // 2 MiB leaf
                    continue;
                }
                if a.tables >= TABLE_BUDGET {
                    a.truncated = true;
                    break 'walk;
                }
                a.tables += 1;
                let pt = (e2 & PTE_ADDR) as *const u64;
                for l in 0..512 {
                    let e1 = unsafe { *pt.add(l) };
                    if e1 & PTE_PRESENT == 0 {
                        continue;
                    }
                    a.record(wx_fold(acc2, e1), 1 << 12, nxe); // 4 KiB leaf
                }
            }
        }
    }
    a
}

/// **WXAUDIT-0 — the negative control.** An audit that only ever prints `user_WX=0` proves nothing:
/// a classifier hard-wired to say "clean" would print the same line. This drives the two pure
/// functions the whole audit rests on with hand-built entries and demands all four verdicts:
///
///   1. a ring-3 W+X leaf under permissive parents **is** flagged (the instrument can fire at all);
///   2. the real U1b B4 code-page shape (USER, read-only, executable) is **not** flagged (no false
///      positive — otherwise the audit would fail every boot and be switched off);
///   3. an NX parent vetoes an executable leaf (the OR-accumulation leg — a leaf-only checker fails this);
///   4. with NXE clear, that same NX-protected page **is** flagged (the vacuity leg — proof the audit
///      really does void its NX conclusions when the hardware is ignoring bit 63).
///
/// Returns true iff all four hold.
pub fn wx_selftest() -> bool {
    // Permissive intermediate: PRESENT|WRITABLE|USER — exactly what `build_slot` writes.
    let inter = PTE_PRESENT | PTE_WRITABLE | PTE_USER;
    let path = |leaf: u64| {
        wx_fold(wx_fold(wx_fold(wx_fold(WX_TOP, inter), inter), inter), leaf)
    };

    // 1. the shape the audit exists to catch: ring-3, writable, executable.
    let bad = path(PTE_PRESENT | PTE_USER | PTE_WRITABLE);
    let fires = bad.0 && wx_violates(bad, true);

    // 2. the real code page: ring-3, read-only, executable. Must stay clean.
    let code = path(PTE_PRESENT | PTE_USER);
    let no_false_positive = code.0 && !wx_violates(code, true);

    // 3. NX on a PARENT must make a writable+executable-looking leaf non-executable.
    let veto = wx_fold(wx_fold(WX_TOP, inter | PTE_NX), PTE_PRESENT | PTE_USER | PTE_WRITABLE);
    let parent_nx_vetoes = !wx_violates(veto, true);

    // 4. ...but only while the hardware honours NX. With NXE clear the same page is W∧X again.
    let vacuity = wx_violates(veto, false);

    fires && no_false_positive && parent_nx_vetoes && vacuity
}

/// Number of ring-3 address spaces `wx_check_slot` has cleared, and a one-shot latch for the armed
/// line (slot builds recur for every spawned child; the ASSERT runs every time, the line does not).
static WX_SLOTS_CHECKED: AtomicU32 = AtomicU32::new(0);
static WX_SLOT_LOGGED: AtomicBool = AtomicBool::new(false);

/// Audit slot `s`'s ring-3 window and refuse to hand it out if any page is both writable and
/// executable. Walks the slot's OWN user branch (`SLOT_PML4[2]`→`SLOT_PDPT`→`SLOT_PD`→`SLOT_PT`)
/// rather than re-walking the shared kernel half for every slot, folding the three intermediates into
/// each leaf exactly as hardware would. Returns `(leaves, violations)`.
///
/// This is the enforcement half of the audit: it runs on every `alloc_user_space`, so no ring-3
/// address space in this kernel is ever dispatched without its W^X shape having been verified against
/// the LIVE table bytes — not against the constants `build_slot` intended to write.
pub fn wx_check_slot(s: usize) -> (u32, u32) {
    assert!(s < USER_SLOTS, "wx_check_slot: slot out of range");
    let nxe = efer_nxe();
    let ui = pml4_index(super::syscall::USER_BASE);
    // SAFETY: the slot tables are `.bss` arrays, identity-mapped; reads only.
    let (e4, e3, e2) = unsafe {
        (
            *slot_pml4_ptr(s).add(ui),
            *slot_pdpt_ptr(s).add(pdpt_index(super::syscall::USER_BASE)),
            *slot_pd_ptr(s).add(pd_index(super::syscall::USER_BASE)),
        )
    };
    if e4 & PTE_PRESENT == 0 || e3 & PTE_PRESENT == 0 || e2 & PTE_PRESENT == 0 {
        return (0, 0); // window not built — nothing ring-3-reachable to violate
    }
    let acc = wx_fold(wx_fold(wx_fold(WX_TOP, e4), e3), e2);
    let (mut leaves, mut viol) = (0u32, 0u32);
    for l in 0..512 {
        // SAFETY: as above — the slot PT is an identity-mapped `.bss` page.
        let e1 = unsafe { *slot_pt_ptr(s).add(l) };
        if e1 & PTE_PRESENT == 0 {
            continue;
        }
        leaves += 1;
        let f = wx_fold(acc, e1);
        if f.0 && wx_violates(f, nxe) {
            viol += 1;
        }
    }
    assert!(
        viol == 0,
        "WXAUDIT: ring-3 window of slot {} has {} page(s) both writable and executable",
        s,
        viol
    );
    let n = WX_SLOTS_CHECKED.fetch_add(1, Ordering::Relaxed) + 1;
    if !WX_SLOT_LOGGED.swap(true, Ordering::Relaxed) {
        serial_println!(
            ":: WXAUDIT-SLOT: ring-3 window W^X verified (slot {}, leaves={}, wx=0, nxe={}) ::",
            s,
            leaves,
            nxe as u8
        );
    }
    let _ = n;
    (leaves, viol)
}

/// Ring-3 address spaces cleared by `wx_check_slot` so far (for a later spec/witness rollup).
pub fn wx_slots_checked() -> u32 {
    WX_SLOTS_CHECKED.load(Ordering::Relaxed)
}

/// **The armed-proof line.** Run the negative control, then audit the live kernel map and publish
/// both. Called once from `arch::init` after `syscall::init` has set EFER.NXE and CR4.SMEP, so the
/// numbers describe an actually-enforcing kernel. Panics if a ring-3-reachable page is W∧X.
pub fn wx_audit_report() {
    if wx_selftest() {
        serial_println!(
            ":: WXAUDIT-0: classifier fires on W+X, clears RO-X, honours parent NX, voids on NXE=0 -> PASS ::"
        );
    } else {
        serial_println!(":: WXAUDIT-0: classifier negative control FAILED -> FAIL ::");
        panic!("WXAUDIT: the W^X classifier failed its own negative control — the audit is not trustworthy");
    }
    // Cost, measured rather than assumed: this walk is now on every boot, in a kernel whose last
    // several arcs were spent buying back boot milliseconds. Raw rdtsc (the APIC/PM-timer rate
    // calibration has not run this early, so cycles is the only honest unit here).
    let t0 = super::now_cycles();
    let a = wx_audit_root(cr3_table() as u64);
    let cycles = super::now_cycles().wrapping_sub(t0);
    // O1 — the leaf histogram, APPENDED (never inserted): every existing `awk` pattern that matched
    // this line before still matches it. `l1/l2/l3` are the 1 GiB / 2 MiB / 4 KiB leaf counts over the
    // WHOLE map, which is what the spared-GiB census in the WXN line structurally cannot see.
    serial_println!(
        ":: WXAUDIT x86: leaves={} user={} user_WX={} kern_WX={} ({} MiB) tables={} nxe={} walk={}kcyc l1={} l2={} l3={}{} ::",
        a.leaves,
        a.user_leaves,
        a.user_wx,
        a.kern_wx,
        a.kern_wx_bytes / (1024 * 1024),
        a.tables,
        a.nxe as u8,
        cycles / 1000,
        a.l1g,
        a.l2m,
        a.l4k,
        if a.truncated { " TRUNCATED" } else { "" }
    );
    // The histogram's own self-check. Every `record` increments `leaves` and exactly one level
    // bucket, so this is an identity — but it is an identity over three separate call sites in the
    // walk, which is precisely the kind of thing a later edit breaks silently. Unlike the three
    // asserts F5 retired from the sweep, this one CAN fire: add a leaf size the `match` does not
    // name, or a `record` that bypasses the histogram, and the boot stops here.
    assert!(
        a.l1g + a.l2m + a.l4k == a.leaves,
        "WXAUDIT: leaf histogram does not sum to the leaf count — l1={} + l2={} + l3={} != leaves={}",
        a.l1g, a.l2m, a.l4k, a.leaves
    );
    assert!(
        a.user_wx == 0,
        "WXAUDIT: {} ring-3-reachable page(s) are both writable and executable",
        a.user_wx
    );
}

// =================================================================================================
// WXPROBE — the read-only reconnaissance the WXN-x86 split needs before it can be designed.
// -------------------------------------------------------------------------------------------------
// WXAUDIT answers "how many leaves are W∧X". It cannot answer the questions a SPLITTER has to answer
// first, because those are facts about SPECIFIC addresses and about the CPU's control state, and the
// census is an aggregate. The split is an edit of a FOREIGN, already-live hierarchy — the firmware's
// (the kernel never builds a page table; it inherits UEFI's across `exit_boot_services`) — so every
// one of these is firmware policy, differs between OVMF and Apple EFI, and is knowable only from a
// boot:
//
//   * LEAF LEVEL at the addresses that matter. Whether the kernel image, the AP trampoline page at
//     physical 0x8000, the framebuffer and ordinary .bss sit under 1 GiB, 2 MiB or 4 KiB leaves
//     decides how many huge leaves the split must break down, and therefore how large a static
//     page-table pool it needs (there is no heap where the split must run — see `arch::init`).
//   * The RAW leaf bits, not just the permission ones. GR15's lesson is that PAT/PCD/PWT are as
//     load-bearing as W and NX: a splitter that rewrites a whole leaf value silently drops the PAT
//     bit and re-UCs the panel. Printing the raw entry means a later reader can re-derive any bit we
//     did not think to name, and the memory-type bits are named explicitly so the fb leaf's typing is
//     visible before and after any future edit. The PAT bit MOVES with the level (bit 7 at 4 KiB,
//     bit 12 at 2 MiB/1 GiB), so it is decoded through `pat_bit_for_level`, never a fixed mask.
//   * The FOLDED permission, not only the leaf's. The audit classifies by the fold down the path; so
//     does the hardware. `fw`/`fx`/`fu` are that fold, computed with the same `wx_fold`/`wx_violates`
//     the census uses, so a probe line and the census line can never disagree about one address.
//   * CR0.WP / CR4.PGE / EFER.NXE. `PGE` decides the FLUSH: changing a parent entry invalidates a
//     gigabyte of translations, and a CR3 reload does NOT evict global entries — so if the firmware
//     left PGE set the split must toggle CR4.PGE, and if it left it clear a CR3 reload suffices and a
//     toggle would newly ENABLE global pages (a semantic change, not a flush). Nothing in this tree
//     has ever read PGE.
//   * `__ehdr_start` + the runtime phdr walk. The kernel is a PIE with no linker script and has no
//     image-bounds symbols at all; the split needs exact per-segment [start,end) + flags to mark
//     .text RX and everything else NX. LLD synthesises `__ehdr_start` at the image base when — and
//     only when — something references it, and the ELF header and phdrs are inside the first PT_LOAD,
//     so the bootloader's segment copy carries them into RAM. This probe is that reference: if the
//     line prints, `rust-lld` under this build's own flags emits the symbol and the mechanism is
//     available to the split; `ok=` says the bytes at that address really are this kernel's ELF
//     header, which is the difference between "the symbol linked" and "the phdrs are readable".
//
// SCOPE. Strictly READ-ONLY: reads of page-table memory, of three control registers, and of the
// kernel's own ELF header. It writes NOTHING — no page-table entry, no control register, no MSR — so
// it cannot perturb the very map it is measuring, and it is safe to leave on every boot. It runs from
// `arch::init` immediately after `wx_audit_report`, i.e. after `syscall::init` armed EFER.NXE and
// CR4.SMEP, so the control-register line describes the same enforcing kernel the census describes.
// Ungated, exactly like WXAUDIT: a reconnaissance line that is absent from the capture you happen to
// be holding is worth nothing, and the whole probe is eight lines and a few hundred reads.
// =================================================================================================

/// Global bit of a page-table entry. Named here rather than in the flag block above because nothing
/// in this tree has ever written it — it is read here purely to see what the firmware chose.
const PTE_GLOBAL: u64 = 1 << 8;

// LLD synthesises `__ehdr_start` at the loaded image base for any ELF link that references it. This
// declaration IS the reference — remove it and the symbol stops being emitted. It is deliberately a
// plain (strong) extern: if this build's linker did not define it the link fails loudly at build
// time, which is a better answer than a runtime `absent` nobody notices.
unsafe extern "C" {
    static __ehdr_start: u8;
}

/// One address's mapping, as the hardware sees it: the leaf entry, the level it terminated at, and
/// the FOLDED `(user, writable, nx)` triple from the whole PML4→PDPT→PD→PT path. `None` if any level
/// on the path is not present. Read-only — the probe twin of `translate`, which returns the physical
/// address and throws the permissions away.
fn wx_probe_leaf(va: u64) -> Option<(u64, u8, (bool, bool, bool))> {
    unsafe {
        let e4 = *cr3_table().add(pml4_index(va));
        if e4 & PTE_PRESENT == 0 {
            return None;
        }
        let acc4 = wx_fold(WX_TOP, e4);
        let e3 = *((e4 & PTE_ADDR) as *const u64).add(pdpt_index(va));
        if e3 & PTE_PRESENT == 0 {
            return None;
        }
        let acc3 = wx_fold(acc4, e3);
        if e3 & PTE_HUGE != 0 {
            return Some((e3, 3, acc3));
        }
        let e2 = *((e3 & PTE_ADDR) as *const u64).add(pd_index(va));
        if e2 & PTE_PRESENT == 0 {
            return None;
        }
        let acc2 = wx_fold(acc3, e2);
        if e2 & PTE_HUGE != 0 {
            return Some((e2, 2, acc2));
        }
        let e1 = *((e2 & PTE_ADDR) as *const u64).add(pt_index(va));
        if e1 & PTE_PRESENT == 0 {
            return None;
        }
        Some((e1, 1, wx_fold(acc2, e1)))
    }
}

/// Emit one `WXPROBE map:` line for `va`. `at` is the stable name a capture is grepped by; every
/// field is present on every line (an unmapped address prints `lvl=none` and zeros) so an `awk`
/// column index means the same thing on every row.
fn wx_probe_addr(at: &str, va: u64, nxe: bool) {
    match wx_probe_leaf(va) {
        Some((e, level, acc)) => {
            let pat = pat_bit_for_level(level);
            serial_println!(
                ":: WXPROBE map: at={} va=0x{:X} lvl={} e=0x{:016X} p={} w={} u={} nx={} g={} pat={} pcd={} pwt={} fw={} fx={} fu={} ::",
                at,
                va,
                match level {
                    3 => "1G",
                    2 => "2M",
                    _ => "4K",
                },
                e,
                (e & PTE_PRESENT != 0) as u8,
                (e & PTE_WRITABLE != 0) as u8,
                (e & PTE_USER != 0) as u8,
                (e & PTE_NX != 0) as u8,
                (e & PTE_GLOBAL != 0) as u8,
                (e & pat != 0) as u8,
                (e & PTE_PCD != 0) as u8,
                (e & PTE_PWT != 0) as u8,
                acc.1 as u8,
                // Executable as the HARDWARE decides it: NX only counts when EFER.NXE is armed —
                // the same vacuity leg the census carries, applied to a single address.
                (!(nxe && acc.2)) as u8,
                acc.0 as u8,
            );
        }
        None => serial_println!(
            ":: WXPROBE map: at={} va=0x{:X} lvl=none e=0x0 p=0 w=0 u=0 nx=0 g=0 pat=0 pcd=0 pwt=0 fw=0 fx=0 fu=0 ::",
            at,
            va
        ),
    }
}

/// Read the kernel's own ELF header and `PT_LOAD` program headers through `__ehdr_start` and print
/// them on one line. Bounded: at most `MAX_SEG` segments are named (this kernel links four), and the
/// count of `PT_LOAD`s actually seen is printed so a truncated list is visible as such.
///
/// Segment vaddrs are LINK-time (`p_vaddr`); the runtime address of a segment is `ehdr + p_vaddr`,
/// which is exact because the image's `min_vaddr` is 0 and UEFI's `allocate_pages` is 4 KiB-aligned,
/// so page alignment survives the load. `ehdr` is on the same line, so no reader has to guess.
fn wx_probe_elf() {
    const MAX_SEG: usize = 4;
    const PT_LOAD: u32 = 1;
    let ehdr = &raw const __ehdr_start as u64;
    // SAFETY: reads only, through a pointer the linker itself resolved to the loaded image base.
    // Every field offset below is architectural ELF64. The magic/class/phentsize checks below are
    // what make the phdr reads safe to perform at all — on any mismatch we print `ok=0` and stop.
    let ok = unsafe {
        let p = ehdr as *const u8;
        core::ptr::read_unaligned(p.cast::<u32>()) == 0x464C_457F // "\x7fELF"
            && *p.add(4) == 2 // ELFCLASS64
            && core::ptr::read_unaligned(p.add(54).cast::<u16>()) == 56 // e_phentsize
    };
    if !ok {
        serial_println!(":: WXPROBE elf: ehdr=0x{:X} ok=0 phnum=0 load=0 ::", ehdr);
        return;
    }
    let (phoff, phnum) = unsafe {
        let p = ehdr as *const u8;
        (
            core::ptr::read_unaligned(p.add(32).cast::<u64>()),
            core::ptr::read_unaligned(p.add(56).cast::<u16>()) as usize,
        )
    };
    let mut seg = [(0u64, 0u64, 0u32); MAX_SEG];
    let mut loads = 0usize;
    for i in 0..phnum.min(64) {
        // SAFETY: `phoff`/`phnum` come from the header validated above; each phdr is 56 bytes inside
        // the first PT_LOAD, which the bootloader copied into RAM along with the header. Reads only.
        let (ptype, flags, vaddr, memsz) = unsafe {
            let ph = (ehdr + phoff + (i as u64) * 56) as *const u8;
            (
                core::ptr::read_unaligned(ph.cast::<u32>()),
                core::ptr::read_unaligned(ph.add(4).cast::<u32>()),
                core::ptr::read_unaligned(ph.add(16).cast::<u64>()),
                core::ptr::read_unaligned(ph.add(40).cast::<u64>()),
            )
        };
        if ptype != PT_LOAD {
            continue;
        }
        if loads < MAX_SEG {
            seg[loads] = (vaddr, memsz, flags);
        }
        loads += 1;
    }
    // `p_flags`: bit 0 = X, bit 1 = W, bit 2 = R. Rendered as a fixed 3-char RWX field so the RX
    // segment (the only one the split may leave executable) is greppable by shape.
    let f = |x: u32| -> [u8; 3] {
        [
            if x & 4 != 0 { b'R' } else { b'-' },
            if x & 2 != 0 { b'W' } else { b'-' },
            if x & 1 != 0 { b'X' } else { b'-' },
        ]
    };
    let s = |i: usize| -> (u64, u64, [u8; 3]) {
        if i < loads.min(MAX_SEG) { (seg[i].0, seg[i].1, f(seg[i].2)) } else { (0, 0, *b"---") }
    };
    let (v0, m0, f0) = s(0);
    let (v1, m1, f1) = s(1);
    let (v2, m2, f2) = s(2);
    let (v3, m3, f3) = s(3);
    let t = |x: [u8; 3]| -> [char; 3] { [x[0] as char, x[1] as char, x[2] as char] };
    let (f0, f1, f2, f3) = (t(f0), t(f1), t(f2), t(f3));
    serial_println!(
        ":: WXPROBE elf: ehdr=0x{:X} ok=1 phnum={} load={} s0=0x{:X}+0x{:X}/{}{}{} s1=0x{:X}+0x{:X}/{}{}{} s2=0x{:X}+0x{:X}/{}{}{} s3=0x{:X}+0x{:X}/{}{}{} ::",
        ehdr, phnum, loads,
        v0, m0, f0[0], f0[1], f0[2],
        v1, m1, f1[0], f1[1], f1[2],
        v2, m2, f2[0], f2[1], f2[2],
        v3, m3, f3[0], f3[1], f3[2],
    );
}

/// **The reconnaissance lines.** Eight of them, emitted from `arch::init` immediately after the
/// WXAUDIT census so the map's aggregate and the specific addresses that shape the split sit
/// together in one capture. Read-only throughout; see the section header for why each fact is here.
pub fn wx_probe_report() {
    use x86_64::registers::control::{Cr0, Cr4};
    const IA32_EFER: u32 = 0xC000_0080;
    let cr0 = Cr0::read_raw() as u64;
    let cr4 = Cr4::read_raw() as u64;
    // SAFETY: reading IA32_EFER is a pure ring-0 MSR read with no side effects.
    let efer = unsafe { x86_64::registers::model_specific::Msr::new(IA32_EFER).read() };
    let nxe = efer & (1 << 11) != 0;
    serial_println!(
        ":: WXPROBE cpu: cr0=0x{:016X} wp={} cr4=0x{:016X} pge={} smep={} smap={} la57={} efer=0x{:016X} nxe={} lme={} ::",
        cr0,
        ((cr0 >> 16) & 1) as u8,  // CR0.WP — what makes the firmware's own table pages read-only
        cr4,
        ((cr4 >> 7) & 1) as u8,   // CR4.PGE — decides PGE-toggle vs CR3-reload for the split's flush
        ((cr4 >> 20) & 1) as u8,  // CR4.SMEP
        ((cr4 >> 21) & 1) as u8,  // CR4.SMAP
        ((cr4 >> 12) & 1) as u8,  // CR4.LA57 — the walkers are 4-level only
        efer,
        nxe as u8,
        ((efer >> 8) & 1) as u8,  // EFER.LME
    );
    // The six addresses the split has to classify. `kimg`/`ktext` bracket the image (base vs a live
    // .text address — a function pointer is the only .text address available without a linker
    // symbol, and it cross-checks the phdr line's R-X segment). `ap8000` is the AP trampoline page,
    // the one genuine execute-outside-the-image in the tree. `fb` is the panel, whose leaf carries
    // the GR15 WC typing. `bss` stands in for the heap, which does not exist yet at `arch::init`
    // (the heap is created ~60 lines later in `kernel_main`) — both are ordinary RAM and get the
    // same blanket NX, so the .bss leaf answers the same question. `lapic` is the one MMIO aperture
    // already live at this point: `apic::init()` ran three calls ago, and no `map_mmio_window`
    // caller (SDHCI, iGPU, Kepler BARs) has run yet.
    wx_probe_addr("kimg", &raw const __ehdr_start as u64, nxe);
    wx_probe_addr("ktext", wx_probe_report as *const () as u64, nxe);
    wx_probe_addr("ap8000", 0x8000, nxe);
    // `try_lock`, not `lock`: this is a diagnostic and must not be able to wedge the boot. On x86
    // `WRITER` is seeded from `BootInfo` in `kernel_main` before `unaos_kernel::init()`, so it is
    // live here; a `base` of 0 would mean the firmware gave us no framebuffer.
    let fb = crate::video::WRITER.try_lock().map(|w| w.base() as u64).unwrap_or(0);
    wx_probe_addr("fb", fb, nxe);
    wx_probe_addr("bss", &raw const SLOT_BACKING as *const u8 as u64, nxe);
    wx_probe_addr("lapic", 0xFEE0_0000, nxe);
    wx_probe_elf();
}

// =================================================================================================
// WXN-x86 M1 — the PDPT-level NX sweep. The first NX bit this kernel has ever put in its own map.
// -------------------------------------------------------------------------------------------------
// WHAT IT DOES, in one sentence: it ORs bit 63 into every present, supervisor-only **PDPT** entry
// except the one or two 1 GiB regions that must stay executable — the kernel image, and the low GiB
// holding the AP trampoline page.
//
// WHY PARENTS AND NOT LEAVES. The hardware's execute permission for a page is the OR of NX down the
// whole PML4→PDPT→PD→PT path, and `wx_fold` (the census's classifier, `:891-897`) folds it the same
// way — so ONE bit on a PDPT entry retires 512 (or 262144) leaves at once, in silicon and in the
// count. On the rMBP that is ~1022 entry writes instead of 66047. The decisive secondary benefit is
// not the arithmetic, it is the blast radius. GR15 (a whole-PTE rewrite that silently dropped the
// panel's PAT bit and cost 8.7–9.1× on the blit path) is avoided structurally here, not by
// discipline. Every write below is a read-modify-write that sets exactly one bit and nothing else.
//
// THE ONE EXCEPTION, and why the invariant is stated carefully (F1). "This sweep never writes a leaf"
// is FALSE in general: a PDPT entry with `PTE_HUGE` set **is** a 1 GiB leaf, and on a firmware whose
// identity map uses 1 GiB leaves the sweep writes them. That write is still correct — marking a 1 GiB
// leaf non-executable is exactly the intent, and it is still a permission-only RMW of bit 63, so no
// memory-type bit and no address bit moves — but it is a LEAF write, and the honest form of the
// invariant is therefore: **this sweep writes bit 63 and nothing else, at whatever level the entry
// terminates.** The loop counts those writes separately as `huge_leaf_nx=` so a capture says whether
// any happened, and the FBWC interlock below is scoped to match: an fb whose leaf IS that 1 GiB entry
// legitimately differs by exactly `PTE_NX` afterwards, and that case is reported, not panicked on.
// Neither known platform reaches it (Boot V: `fb lvl=2M`, `lapic lvl=2M`; OVMF likewise), which is
// why it is a latent defect and not a fire — and why `huge_leaf_nx=0` is the expected reading on both.
//
// WHERE IT RUNS, and why that is not negotiable. `arch::init`, between `syscall::init()` and
// `wx_audit_report()`:
//   * AFTER `syscall::init` because with EFER.NXE clear, bit 63 is RESERVED, not ignored — setting
//     it before NXE is armed faults on the next translation, of any kind. The sweep re-reads NXE
//     itself and REFUSES rather than trusting the call site.
//   * BEFORE `wx_audit_report` so the existing WXAUDIT census line IS the success signature. No
//     second audit call, no reader having to work out which of two numbers is live.
//   * BEFORE `wx_probe_report` so the WXPROBE `map:` lines are a post-sweep readback. Their `e=`
//     fields must be **bit-identical** to the pre-sweep metal capture (Boot V: fb `e=0x900010E3`,
//     ap8000 `e=0x8003`) — which is the strongest available statement that no leaf was touched —
//     while their folded `fx=` fields flip to 0 everywhere outside the spared GiBs, which is the
//     statement that the parents did change. One instrument, two independent claims.
//   * BEFORE `sti`, and inside `with_page_tables_writable`, which masks interrupts anyway.
//   * It needs no allocator, which is what lets it sit here at all: a PDPT-level sweep creates no
//     tables, and the heap does not exist until ~60 lines later in `kernel_main`.
//
// WHAT M1 DELIBERATELY DOES NOT DO. It does not split a huge leaf (M2), it does not clear W on
// `.text` (M3), and it does not set CR0.WP. That last one matters on metal: the rMBP's firmware
// leaves **CR0.WP=0** (Boot V; QEMU leaves it 1), so until M3 arms WP deliberately a read-only PTE
// bit does not bind ring 0 at all. NX enforcement does NOT depend on WP — it is EFER.NXE and bit 63
// and nothing else — so this milestone is fully real on metal; but the RO half of W^X is not yet,
// and `WXAUDIT-NXE`'s `wp_mask` says so on the wire. Setting WP is M3's job because it changes what
// `with_page_tables_writable` means for the copy paths M3 introduces.
//
// THE COUNTER-HAZARD, stated once so the next arc inherits it rather than rediscovers it: an NX'd
// non-leaf entry silently vetoes any FUTURE executable mapping created beneath it. Nothing in the
// tree creates one today (`map_mmio_window` mints NX leaves; ring-3 lives under a separate PML4
// slot at 1 TiB), but a later arc that maps executable memory under an identity GiB will find it
// non-executable for reasons no leaf can explain.
// =================================================================================================

/// The most 1 GiB regions the sweep will ever spare: the trampoline's, plus the image's (which is
/// ~6.7 MiB and can therefore straddle at most two). Four is slack, and overflowing it is a named
/// panic rather than a silently under-spared map.
const WXN_MAX_SPARE: usize = 4;

/// Page tables the residue census will descend into per spared GiB before it gives up and says so.
/// The census is a diagnostic, not a correctness leg; it must not be able to cost real boot time.
const WXN_PT_CENSUS_CAP: u32 = 64;

/// The 1 GiB regions left executable, as `va >> 30` indices. Tiny fixed-capacity set — no allocator
/// exists where this runs.
struct WxnSpare {
    gib: [u64; WXN_MAX_SPARE],
    n: usize,
}

impl WxnSpare {
    const fn new() -> Self {
        WxnSpare { gib: [0; WXN_MAX_SPARE], n: 0 }
    }
    fn contains(&self, g: u64) -> bool {
        let mut i = 0;
        while i < self.n {
            if self.gib[i] == g {
                return true;
            }
            i += 1;
        }
        false
    }
    /// Returns false only on capacity overflow (already-present is a success).
    fn add(&mut self, g: u64) -> bool {
        if self.contains(g) {
            return true;
        }
        if self.n == WXN_MAX_SPARE {
            return false;
        }
        self.gib[self.n] = g;
        self.n += 1;
        true
    }
}

/// Runtime `[start, end)` of the loaded kernel image, from its own ELF header and `PT_LOAD` phdrs
/// through `__ehdr_start`. `None` if the bytes at `__ehdr_start` are not a well-formed ELF64 header
/// or carry no `PT_LOAD` — in which case the sweep refuses to run rather than guess at bounds.
///
/// The image is a PIE with `min_vaddr == 0`, so a segment's runtime address is `ehdr + p_vaddr`
/// exactly; UEFI's `allocate_pages` is 4 KiB-aligned, so page alignment survives the load.
fn wxn_image_bounds() -> Option<(u64, u64)> {
    const PT_LOAD: u32 = 1;
    let ehdr = &raw const __ehdr_start as u64;
    // SAFETY: reads only, through a pointer the linker resolved to the loaded image base. The magic
    // / class / phentsize checks are what make the phdr reads below safe to perform at all.
    let ok = unsafe {
        let p = ehdr as *const u8;
        core::ptr::read_unaligned(p.cast::<u32>()) == 0x464C_457F // "\x7fELF"
            && *p.add(4) == 2 // ELFCLASS64
            && core::ptr::read_unaligned(p.add(54).cast::<u16>()) == 56 // e_phentsize
    };
    if !ok {
        return None;
    }
    let (phoff, phnum) = unsafe {
        let p = ehdr as *const u8;
        (
            core::ptr::read_unaligned(p.add(32).cast::<u64>()),
            core::ptr::read_unaligned(p.add(56).cast::<u16>()) as usize,
        )
    };
    let mut lo = ehdr; // the header itself is inside the first PT_LOAD, at p_vaddr 0
    let mut hi = 0u64;
    for i in 0..phnum.min(64) {
        // SAFETY: `phoff`/`phnum` come from the header validated above; each phdr is 56 bytes inside
        // the first PT_LOAD, which the bootloader copied into RAM along with the header.
        let (ptype, vaddr, memsz) = unsafe {
            let ph = (ehdr + phoff + (i as u64) * 56) as *const u8;
            (
                core::ptr::read_unaligned(ph.cast::<u32>()),
                core::ptr::read_unaligned(ph.add(16).cast::<u64>()),
                core::ptr::read_unaligned(ph.add(40).cast::<u64>()),
            )
        };
        if ptype != PT_LOAD || memsz == 0 {
            continue;
        }
        lo = lo.min(ehdr + vaddr);
        hi = hi.max(ehdr + vaddr + memsz);
    }
    if hi <= lo { None } else { Some((lo, hi)) }
}

/// Count the present leaves inside the spared 1 GiB regions — i.e. the residue the sweep is about to
/// LEAVE behind, computed independently of the census that will report it.
///
/// This is the milestone's self-prediction. `kern_WX` on the next line of the log must equal this
/// number (minus any leaf the firmware already left read-only), and the two counts come from two
/// separate walks of the same tables. A sweep that missed entries disagrees with itself, on one
/// screen, without anyone having to hold a baseline in their head.
///
/// **The metal prediction, stated with its actual precision.** Boot V's baseline
/// (`leaves=66047`, `131072 MiB`, `tables=1028`, and `kern_WX == leaves`, i.e. no RO/NX residue) does
/// **not** pin the leaf mix: the whole family `n1 = k, n4 = 512(k+1), n2 = 65536 − 512k − (k+1)`
/// satisfies all three numbers for every k ≥ 0, because the byte total is degenerate in k. So the
/// residue this census will print is **1535 or 2046** — 1535 under k = 0 (the natural reading:
/// `GiB0 = 511×2M + 512×4K`, `GiB1 = 512×2M`), 2046 if a second page table also lies inside the
/// spared GiBs, as k = 1 permits. Which one lands is decided ON THE BOOT, by two things this arc now
/// prints rather than assumes: `(1g= 2m= 4k= pt=)` here, and the whole-map `l1=/l2=/l3=` histogram on
/// the WXAUDIT line (O1). `pdpt_seen=1024`, `nx_set=1022` and `skip_spare=2` are invariant across the
/// family and stay hard predictions.
///
/// Returns `(leaves, 1G leaves, 2M leaves, 4K leaves, page tables descended, capped)`.
fn wxn_spare_census(spare: &WxnSpare) -> (u32, u32, u32, u32, u32, bool) {
    let (mut l1g, mut l2m, mut l4k, mut pts) = (0u32, 0u32, 0u32, 0u32);
    let mut capped = false;
    for s in 0..spare.n {
        let g = spare.gib[s];
        let va = g << 30;
        // SAFETY: every page-table frame is identity-mapped and directly readable at ring 0. Reads
        // only — this runs before the sweep and must not perturb what it is measuring.
        unsafe {
            let e4 = *cr3_table().add(pml4_index(va));
            if e4 & PTE_PRESENT == 0 {
                continue;
            }
            let e3 = *((e4 & PTE_ADDR) as *const u64).add(pdpt_index(va));
            if e3 & PTE_PRESENT == 0 {
                continue;
            }
            if e3 & PTE_HUGE != 0 {
                l1g += 1;
                continue;
            }
            let pd = (e3 & PTE_ADDR) as *const u64;
            for k in 0..512 {
                let e2 = *pd.add(k);
                if e2 & PTE_PRESENT == 0 {
                    continue;
                }
                if e2 & PTE_HUGE != 0 {
                    l2m += 1;
                    continue;
                }
                if pts >= WXN_PT_CENSUS_CAP {
                    capped = true;
                    continue;
                }
                pts += 1;
                let pt = (e2 & PTE_ADDR) as *const u64;
                for l in 0..512 {
                    if *pt.add(l) & PTE_PRESENT != 0 {
                        l4k += 1;
                    }
                }
            }
        }
    }
    (l1g + l2m + l4k, l1g, l2m, l4k, pts, capped)
}

/// **The sweep.** See the section header for the full argument. Called once, from `arch::init`,
/// between `syscall::init()` and `wx_audit_report()`.
///
/// Refuses (loudly, without writing anything) if EFER.NXE is clear or if the image bounds cannot be
/// derived. Panics if its own self-checks fail — a boot-breaking panic is the correct outcome for
/// "I am about to mark the code I am executing non-executable", and for "the framebuffer leaf
/// changed under a sweep that writes no leaves".
pub fn wxn_pdpt_sweep() {
    use x86_64::registers::control::{Cr0, Cr4};
    const CR4_PGE: u64 = 1 << 7;
    // The 1 GiB region holding the AP trampoline page, from `smp`'s own constant — not a second
    // hard-coded 0x8000, so a retargeted SIPI vector cannot leave this sweep sparing the wrong GiB.
    let tramp_gib = (super::smp::TRAMPOLINE_ADDR as u64) >> 30;

    let cr0 = Cr0::read_raw() as u64;
    let cr4 = Cr4::read_raw() as u64;
    let wp = (cr0 >> 16) & 1;
    let pge = cr4 & CR4_PGE != 0;

    // Leg 1 — fail closed on NXE. The call site already orders this after `syscall::init`, but an
    // ordering argument in a comment is not a check: with NXE clear, bit 63 is a RESERVED bit and
    // every entry we set would fault the next translation through it. Ask the MSR, not the caller.
    if !efer_nxe() {
        serial_println!(
            ":: WXN-x86: nxe=0 -> REFUSED (bit 63 is RESERVED with EFER.NXE clear; no entry written) ::"
        );
        return;
    }

    // Leg 2 — fail closed on bounds. Without exact image bounds the sweep cannot know which GiB to
    // spare, and a wrong guess makes the kernel non-executable. No bounds, no sweep.
    let Some((img_lo, img_hi)) = wxn_image_bounds() else {
        serial_println!(
            ":: WXN-x86: ehdr=0x{:X} elf=bad -> REFUSED (no image bounds; no entry written) ::",
            &raw const __ehdr_start as u64
        );
        return;
    };

    let mut spare = WxnSpare::new();
    let mut fits = spare.add(tramp_gib);
    let mut g = img_lo >> 30;
    while g <= (img_hi - 1) >> 30 {
        fits &= spare.add(g);
        g += 1;
    }

    // Leg 2b — fail closed on spare-set overflow. This is a REFUSAL, not an assert, and it is not a
    // claim of coverage: with a ~6.7 MiB image the spare set is {tramp} ∪ {at most 2 image GiBs}, so
    // `n ≤ 3 < WXN_MAX_SPARE = 4` and `fits` cannot be false short of a ≥ 2 GiB kernel. It is kept
    // because an under-spared set is the one failure that makes the running kernel non-executable,
    // and because the checks below are only implied by the bounds check *given* that the loop
    // inserted every index in the range — which is exactly what `fits` states. Refusing (no entry
    // written, one loud line) is strictly better than a panic: the boot survives, unprotected and
    // visibly so.
    if !fits {
        serial_println!(
            ":: WXN-x86: img=[0x{:X},0x{:X}) spare_cap={} -> REFUSED (image spans more 1 GiB regions \
             than WXN_MAX_SPARE; no entry written) ::",
            img_lo, img_hi, WXN_MAX_SPARE
        );
        return;
    }

    // Leg 3 — the self-check that wrong bounds cannot survive, and THE one load-bearing assert in
    // this function. `wxn_pdpt_sweep` is itself a `.text` address, so it must lie inside the derived
    // image span. If the phdr walk latched onto the wrong ELF header (an `include_bytes!` ring-3 blob
    // in `.rodata`, a truncated walk), this fires here — before a single entry is written — instead
    // of as a dead machine one instruction after the flush.
    //
    // F5 — two asserts that used to stand here have been REMOVED rather than left as false coverage:
    //   * `spare.contains(here >> 30)` could not fail once this assert passed. `img_lo ≤ here < img_hi`
    //     puts `here >> 30` inside the closed range `[img_lo >> 30, (img_hi − 1) >> 30]`, and the
    //     `while` loop above inserted EVERY index in that range (with `fits` already established).
    //   * `spare.contains(tramp_gib)` was a tautology: `spare.add(tramp_gib)` is the first insertion
    //     into an empty fixed-capacity set, so it always succeeds and `contains` always returns true.
    // Both read as coverage of a hazard nothing could produce. The hazards are real; the checks were
    // not checking them. What actually protects the trampoline GiB is that it is inserted first and
    // unconditionally; what actually protects `.text` is the bounds assert immediately below.
    let here = wxn_pdpt_sweep as *const () as u64;
    assert!(
        here >= img_lo && here < img_hi,
        "WXN-x86: derived image bounds [{:#x},{:#x}) do not contain this function ({:#x}) — the phdr \
         walk found the wrong ELF header",
        img_lo, img_hi, here
    );

    // The residue prediction, computed BEFORE the sweep (the sweep changes no PD/PT contents, so the
    // number is the same either way; taking it first keeps the edit window as short as possible).
    let (sp_leaves, sp_1g, sp_2m, sp_4k, sp_pts, sp_capped) = wxn_spare_census(&spare);

    // R2 / GR15 — the framebuffer leaf, read before and read back after. The argument that this
    // sweep cannot disturb the panel's WC typing is airtight (a non-leaf entry's PWT/PCD affect only
    // the caching of the page-table WALK, and bit 63 affects nothing but fetch permission), but GR15
    // is exactly what happens when an argument is the only belt: the panel was uncached for two
    // weeks behind a correct-sounding sentence. So: capture the raw leaf value, and PANIC on any
    // change. A boot-breaking panic naming the two values is strictly better than a silently
    // un-typed panel, which costs 8.7–9.1× on the blit path and is invisible to every permission
    // instrument in the tree, WXAUDIT included.
    //
    // F2 — the three ways this interlock can silently not happen, each counted on the WXN line so a
    // capture without a `WXN-FBWC:` line is never indistinguishable from a build without the
    // interlock: `try_lock` contention (`skip_fb_lock`), a firmware that gave us no framebuffer
    // (`skip_fb_base`), and an fb address the walk cannot resolve (`skip_fb_walk`). `try_lock`, not
    // `lock`: a diagnostic must not be able to wedge the boot, and the guard is dropped before any
    // `serial_println!` below (the fbcon mirror takes the same lock).
    //
    // And the honest scope, so no later reader over-reads a green line: with respect to the GR15
    // hazard it names this check is vacuously green BY CONSTRUCTION whenever `huge_leaf_nx == 0` —
    // no leaf was written, so no leaf value can have changed. What it genuinely discriminates is a
    // LEVEL CONFUSION: a loop that wrote PD entries instead of PDPT entries would hit the fb's own PD
    // and fire. Keep it as that interlock; it is not a safety leg.
    let (fb, skip_fb_lock) = match crate::video::WRITER.try_lock() {
        Some(w) => (w.base() as u64, 0u32),
        None => (0u64, 1u32),
    };
    let skip_fb_base = u32::from(skip_fb_lock == 0 && fb == 0);
    let fb_before = if fb != 0 { wx_probe_leaf(fb).map(|(e, l, _)| (e, l)) } else { None };
    let skip_fb_walk = u32::from(fb != 0 && fb_before.is_none());

    let (mut pdpt_seen, mut nx_set, mut skip_spare) = (0u32, 0u32, 0u32);
    let (mut skip_user, mut skip_pml4_user, mut already_nx, mut skip_selfmap) = (0u32, 0u32, 0u32, 0u32);
    // F1 — PDPT entries that are themselves 1 GiB LEAVES and got bit 63. Counted apart from
    // `nx_set` (which stays the total number of entries written, so its prediction is unchanged)
    // because these, and only these, are leaf writes.
    let mut huge_leaf_nx = 0u32;

    let root = cr3_table();
    let root_pa = root as u64;
    // On metal CR0.WP is already 0, so this wrapper is a no-op there; on QEMU (WP=1) the firmware's
    // table pages are read-only to ring 0 and it is what makes the stores land. It also masks
    // interrupts for the whole edit, which is free here (`sti` has not run yet).
    with_page_tables_writable(|| {
        // SAFETY: every page-table frame is identity-mapped and directly addressable at ring 0.
        // Every store below is `e | PTE_NX` — a read-modify-write that sets exactly bit 63 and
        // preserves the address field, the memory-type bits and every permission bit the firmware
        // chose. No whole-entry value is ever written (GR15's law).
        unsafe {
            for i in 0..512usize {
                let e4 = core::ptr::read_volatile(root.add(i));
                if e4 & PTE_PRESENT == 0 {
                    continue;
                }
                // D10: never touch a PML4 entry (`build_slot` copies those into every ring-3 slot at
                // build time, so a PML4 edit would be invisible to live slots), and never descend
                // into a user-reachable one.
                if e4 & PTE_USER != 0 {
                    skip_pml4_user += 1;
                    continue;
                }
                let child = e4 & PTE_ADDR;
                if child == root_pa {
                    // Recursive self-map: "PDPT entries" under it are the PML4's own entries, and
                    // writing NX there would veto the entire address space. Neither Apple EFI nor
                    // OVMF builds one (`tables=1028`/`1034` prove it — a self-map would explode the
                    // walk), but the guard costs one comparison and the failure mode is total.
                    skip_selfmap += 1;
                    continue;
                }
                let pdpt = child as *mut u64;
                for j in 0..512usize {
                    let p = pdpt.add(j);
                    let e3 = core::ptr::read_volatile(p);
                    if e3 & PTE_PRESENT == 0 {
                        continue;
                    }
                    pdpt_seen += 1;
                    if e3 & PTE_USER != 0 {
                        // Ring-3-reachable subtree: not ours to restrict here, and the ring-3 W^X
                        // legs own it. `user=0` on the census means this counter should stay 0 too;
                        // a non-zero value would say the sweep is skipping real identity memory.
                        skip_user += 1;
                        continue;
                    }
                    if spare.contains(((i as u64) << 9) | j as u64) {
                        skip_spare += 1;
                        continue;
                    }
                    if e3 & PTE_NX != 0 {
                        already_nx += 1;
                        continue;
                    }
                    // F1 — a PDPT entry with PTE_HUGE IS a 1 GiB leaf. Setting NX on it is right
                    // (that is exactly the region we mean to make non-executable) and the store is
                    // the same permission-only RMW as any other — bit 63 and nothing else, so no
                    // memory-type bit, no address bit and no Global bit moves, and GR15's law holds.
                    // But it is a LEAF write, so it is counted apart: `huge_leaf_nx > 0` is what
                    // tells a reader that the "no leaf was written" reading of the FBWC interlock
                    // below does not apply on this platform.
                    if e3 & PTE_HUGE != 0 {
                        huge_leaf_nx += 1;
                    }
                    core::ptr::write_volatile(p, e3 | PTE_NX);
                    nx_set += 1;
                }
            }
        }
    });

    // D6 — the flush, and the branch on the wire. See `wxn_flush_tlb`: the body moved there verbatim
    // when M2 needed the identical branch, so both milestones flush the same way and the `flush=`
    // token on both lines names one implementation.
    wxn_flush_tlb(pge, cr4);

    // F6 — the verdict. Every other WXN instrument in this arc carries one; without it a sweep that
    // wrote NOTHING prints a line that reads entirely normal. The concrete way that happens: if the
    // firmware set U/S on its identity PML4 entries, every descent takes the `skip_pml4_user` branch,
    // `pdpt_seen` stays 0 and no entry is touched — the milestone is completely vacuous and only a
    // reader who cross-reads the NEXT line's `kern_WX` would notice. So: `nx_set == 0` while the map
    // does have PDPT entries that are not all spared is VACUOUS, on the wire, in one greppable token.
    // (The failure is fail-safe — the sweep under-protects, it can never over-protect — which is why
    // this is an honesty gap and not a correctness one.)
    // `pdpt_seen == 0` is the U/S case above (the sweep never reached a PDPT entry at all);
    // `spare_n < pdpt_seen` is the case where entries existed beyond the ones we deliberately spared
    // and still none was written. Either way this sweep wrote nothing. (`already_nx == pdpt_seen`
    // would also land here — a map the firmware had already NX'd. That map is protected, just not by
    // us, and `already_nx=` on this same line says which of the two it was.)
    let vacuous = nx_set == 0 && (pdpt_seen == 0 || (spare.n as u32) < pdpt_seen);
    let verdict = if vacuous { "-> VACUOUS" } else { "-> SWEPT" };
    serial_println!(
        ":: WXN-x86: ehdr=0x{:X} img=[0x{:X},0x{:X}) gib_img={} gib_tramp={} spare_n={} pdpt_seen={} nx_set={} \
         huge_leaf_nx={} skip_spare={} skip_user={} skip_pml4_user={} skip_selfmap={} already_nx={} \
         skip_fb_lock={} skip_fb_base={} skip_fb_walk={} residue_leaves={} \
         (1g={} 2m={} 4k={} pt={}{}) pge={} flush={} wp={} {} ::",
        &raw const __ehdr_start as u64,
        img_lo,
        img_hi,
        img_lo >> 30,
        tramp_gib,
        spare.n,
        pdpt_seen,
        nx_set,
        huge_leaf_nx,
        skip_spare,
        skip_user,
        skip_pml4_user,
        skip_selfmap,
        already_nx,
        skip_fb_lock,
        skip_fb_base,
        skip_fb_walk,
        sp_leaves,
        sp_1g,
        sp_2m,
        sp_4k,
        sp_pts,
        if sp_capped { " CAPPED" } else { "" },
        pge as u8,
        if pge { "pge-toggle" } else { "cr3-reload" },
        wp,
        verdict,
    );

    // The GR15 belt, cashed. `residue_leaves` above is what the WXAUDIT line on the next screen must
    // report as `kern_WX` (less any leaf the firmware already left read-only); `WXN-FBWC` here is
    // what says the panel survived. `fx=0` is the sweep working — the fb is now non-executable via
    // its PDPT parent — while `e=` unchanged is the sweep having touched no leaf to achieve it.
    if let Some((e_before, l_before)) = fb_before {
        match wx_probe_leaf(fb) {
            Some((e_after, l_after, acc)) => {
                // The level must never move: this sweep creates and destroys no table, so the fb's
                // walk must terminate at the same level it did before, whatever that level is.
                assert!(
                    l_after == l_before,
                    "WXN-x86: the framebuffer walk terminated at a DIFFERENT LEVEL across the sweep \
                     — before=0x{:016X}/lvl{} after=0x{:016X}/lvl{}. The sweep creates no table and \
                     destroys none, so the map changed shape underneath it.",
                    e_before, l_before, e_after, l_after
                );
                // F1 — the entry compare, scoped to what the sweep can legitimately have done at the
                // level the walk ACTUALLY terminated at. Two outcomes are correct, and they are not
                // the same claim:
                //   * `delta == 0` — the fb leaf is below the PDPT (lvl 1 or 2, both known
                //     platforms), the sweep wrote only parents, and nothing about the leaf moved.
                //   * `delta == PTE_NX` at lvl 3 — the fb IS a 1 GiB leaf and this sweep set bit 63
                //     on it. That is the sweep working as designed on such a firmware, not a GR15
                //     event, and the old unconditional compare would have PANICKED THE BOOT here with
                //     a message blaming a defect that had not occurred.
                // Anything else — any other bit, or a bit-63 change at a level this sweep never
                // writes — is the GR15 class (a dropped PAT bit silently re-UCs the panel, 8.7-9.1x
                // on the blit path, invisible to every permission instrument) and stops the boot.
                let delta = e_after ^ e_before;
                let huge_leaf_sweep = l_after == 3 && delta == PTE_NX && e_after & PTE_NX != 0;
                assert!(
                    delta == 0 || huge_leaf_sweep,
                    "WXN-x86: the framebuffer LEAF changed in bits other than PTE_NX — \
                     before=0x{:016X} after=0x{:016X} delta=0x{:016X} lvl={}. This is the GR15 defect \
                     (a dropped PAT/PCD/PWT bit silently re-UCs the panel, 8.7-9.1x on the blit path) \
                     and it is invisible to every permission instrument, so the boot stops here.",
                    e_before, e_after, delta, l_after
                );
                let pat = pat_bit_for_level(l_after);
                serial_println!(
                    ":: WXN-FBWC: fb=0x{:X} lvl={} e=0x{:016X} pat={} pcd={} pwt={} w={} fx={} {} ::",
                    fb,
                    l_after,
                    e_after,
                    (e_after & pat != 0) as u8,
                    (e_after & PTE_PCD != 0) as u8,
                    (e_after & PTE_PWT != 0) as u8,
                    (e_after & PTE_WRITABLE != 0) as u8,
                    (!acc.2) as u8,
                    if delta == 0 {
                        "-> LEAF BIT-IDENTICAL"
                    } else {
                        "-> LEAF NX-ONLY (fb is a 1G leaf this sweep NX'd; expected)"
                    },
                );
            }
            None => panic!(
                "WXN-x86: the framebuffer mapping at {:#x} is gone after the sweep — it was present \
                 (leaf 0x{:016X}) before it",
                fb, e_before
            ),
        }
    } else {
        // F2 — the interlock did not run, and says so. Without this line a capture with no
        // `WXN-FBWC:` in it is indistinguishable from a build that never had the tripwire; the
        // `skip_fb_*` fields on the WXN line above name which of the three paths it took.
        serial_println!(
            ":: WXN-FBWC: fb=0x{:X} skip_lock={} skip_base={} skip_walk={} -> SKIPPED ::",
            fb, skip_fb_lock, skip_fb_base, skip_fb_walk
        );
    }

    // M2 — the splitter. Runs LAST, after the M1 verdict line and after the FBWC interlock has been
    // cashed, so every M1 instrument above measures exactly the M1 sweep and nothing else: the
    // interlock's "the sweep creates no table and destroys none" level assert stays literally true of
    // the window it brackets. M2 carries its own fb interlock (`fb_delta=` on its own line) and its
    // own `leaf_is_fb_wc` refusal, and `wx_probe_report` — which runs after BOTH — is the post-M2
    // leaf readback for every named address.
    wxn_split_stage(&spare, img_lo, img_hi, pge, cr4);

    // M3b — the W-clear. AFTER M2, because it needs the 4 KiB granularity M2 created (a 2 MiB leaf
    // straddling the extent would otherwise force it to refuse), and after M2's fb interlock has been
    // cashed so each stage brackets its own window. Still inside `arch::init`: interrupts masked,
    // pre-`sti`, and — the property that costs nothing and buys the most — before `smp::start_aps`,
    // so no AP can ever have cached a writable translation of a page this clears. `wx_probe_report`,
    // which runs after all three, is the post-M3b leaf readback: `at=ktext` is the address whose
    // `w=`/`fw=` fields this stage is expected to move from 1 to 0.
    wxn_ro_stage(pge, cr4);
}

// =================================================================================================
// WXN-x86 M2 — the huge-leaf splitter, the static page-table pool, and NX inside the spared GiBs.
// -------------------------------------------------------------------------------------------------
// WHAT M1 LEFT. The PDPT sweep retired every 1 GiB region except the one or two it had to spare: the
// kernel image's, and the low GiB holding the AP trampoline. Those spared GiBs are still RWX in full
// — 1535 leaves on metal (Boot W: GiB0 = 511x2M + 512x4K, GiB1 = 512x2M), 4089 on QEMU (all of GiB0).
// M2 shrinks that residue to the pages that genuinely need X.
//
// WHAT "NEEDS X", and why the obvious answer is WRONG on this kernel. Three readings were available:
//   (A) the whole image extent `[img_lo, img_hi)` — simplest, leaves `.data`/`.bss` executable;
//   (B) the union of the image's `PF_X` `PT_LOAD` segments — `.text` and nothing else; or
//   (C) the union of the image's NON-WRITABLE `PT_LOAD` segments — `.rodata` and `.text`.
// (B) is what the design's M2 row assumes (`kern_WX` = `.text` pages + 1 ~ 253) and it is what this
// code shipped first. **It was falsified on the first boot**, and the falsification is the most
// valuable thing this milestone produced:
//
//     EXCEPTION: PAGE FAULT  err=PROTECTION_VIOLATION|INSTRUCTION_FETCH  rip=0x3D646C68
//
// — image offset 0x27C68, which `readelf -sW` names **`switch_context`**, sitting inside `.rodata`.
// The cause is one missing directive: `smp.rs`'s `global_asm!` opens `.section .rodata` for the AP
// trampoline bytes and never returns to `.text`. Rust concatenates every `global_asm!` in a crate into
// ONE assembly unit, so the section state leaks — `switch_context`, assembled after it, lands in
// `.rodata` and is EXECUTED IN PLACE at ring 0 on every task switch. `readelf` confirms the two are
// adjacent: `ap_trampoline_end` and `switch_context` share the address 0x27C68.
//
// So `PF_X` is not the bit that tells the truth about this image; `PF_W` is. **This code implements
// (C)**: X is kept exactly where the image declares the pages NOT writable. That is:
//   * correct by measurement rather than by assumption — it covers `.rodata`, which this boot proved
//     holds live ring-0 code, and it would cover any future `.section` leak of the same shape;
//   * still a real confinement — the `.data`/`.bss`/`.got` LOADs (5.9 MiB, the bulk of the image, and
//     the only part of it that is genuinely writable) go NX, along with all 126 other GiBs from M1; and
//   * honest about W^X: a page the ELF itself marks writable can never legitimately hold code we
//     execute in place, so `PF_W` is exactly the discriminator the property is about.
// It is NOT the end state. The `.rodata` leak is a real defect and `switch_context` belongs in
// `.text`; fixing it lives in `smp.rs`, outside this arc's file. Until it is fixed, M3 cannot make
// `.text` read-only-executable and call the job done — it would have to do the same for `.rodata`,
// which is the wrong shape. **Flagged for the next arc, with the symbol named.**
//
// The genuinely risky flip — clearing W on `.text`, where a single runtime writer to a `.text` data
// symbol turns into a `#PF` — is untouched and stays M3's, alone in its own commit, exactly as the
// design requires for a clean bisect. M2 changes X only. `.text` is still writable here.
// The one new failure mode this introduces is a wrong extent marking the running `.text` NX. That is
// closed the same way M1 closed wrong image bounds: an assert that this very function's address lies
// inside the derived extent, evaluated BEFORE a single entry is written.
//
// THE POOL, and why there is one. M2 runs where M1 runs — `arch::init`, before `sti`, ~60 lines
// before `kernel_main` creates the heap — so `alloc_page_frame` does not exist yet. The frames come
// from `.bss` through the same identity trick the ring-3 slot tables use (`table_pa`: an identity-
// mapped `.bss` page's address IS its physical address). Sizing, derived rather than guessed:
//   * spared GiBs <= `WXN_MAX_SPARE` = 4 (M1 refuses above that), so at most 4 of them can be 1 GiB
//     leaves needing a PD to demote into                                              => <= 4 PDs
//   * PTs are needed only for the 2 MiB leaves that STRADDLE the keep set — a leaf wholly inside it
//     keeps X untouched and a leaf wholly outside it takes one NX bit at the PD level. The keep set
//     is two intervals, so at most 2 leaves straddle per interval boundary: the `PF_X` extent
//     contributes `ceil(|.text| / 2 MiB) + 1` and the trampoline page contributes 1. With a 1 MiB
//     `.text` that is 2 + 1 = 3; budgeting for a 16 MiB `.text` gives 9 + 1  => <= 10 PTs
//   * total <= 14. `WXN_POOL_CAP` = 16 is that bound plus slack, and costs 64 KiB of `.bss` —
//     1.5% of what `SLOT_BACKING` already spends.
// Exhaustion is FAIL-CLOSED and it is decided BEFORE the edit window opens: a read-only pre-pass
// counts the exact number of tables the edit will consume, and if that exceeds the pool M2 prints one
// REFUSED line and writes nothing at all. There is no partially-split map to reason about. (The
// `take()` inside the window therefore cannot fail; if it ever did it would mean the pre-pass and the
// edit disagree about the map, which is a panic, not a refusal.)
//
// THE BIT-CARRY, i.e. the GR15 trap. Splitting is the one operation in this file that writes WHOLE
// entry values, so it is the one place the panel's PAT bit can be dropped. Two facts make it a trap:
//   * the PAT bit MOVES with the level — bit 12 on a 2 MiB/1 GiB leaf, bit 7 on a 4 KiB PTE — so a
//     copy that preserves "all the low bits" moves PAT into PS and PS into PAT; and
//   * `PTE_ADDR` (bits 51:12) is NOT the address field of a huge leaf. A 2 MiB leaf's base is bits
//     51:21 and a 1 GiB leaf's is 51:30; masking with `PTE_ADDR` would fold the PAT bit into the
//     address.
// So the carry is explicit and enumerated (`WXN_LEAF_CARRY`), the huge-leaf address masks are their
// own constants, and the PAT bit is translated by hand at the one place the level changes. On top of
// that: the splitter REFUSES (named panic) to split any leaf `leaf_is_fb_wc` recognises, and M2 reads
// the fb leaf back across its own edit window and panics on any delta other than `PTE_NX`.
//
// THE ORDER OF WRITES, and why it is not the design's. The design's M2 says "populate the new table ->
// store over the old leaf -> invlpg -> only then edit individual entries". This code populates the new
// table with its FINAL values (NX already set on every page outside the keep set) before the store,
// which is strictly stronger: the 2 MiB region is never observable in a half-restricted state, and the
// only stale translation the TLB can serve between the store and the flush is the OLD, MORE PERMISSIVE
// huge entry — a vacuity, never a fault. The per-split `invlpg` stride sweep is kept as a belt; the
// CR3-reload / PGE-toggle at the end (`wxn_flush_tlb`, shared with M1) is what actually guarantees it.
//
// AP SAFETY. On Apple EFI 0x8000 is already a 4 KiB leaf (Boot W: GiB0 carries a 512-entry PT), so the
// trampoline's PD entry is a table pointer and M2 edits its 512 PTEs in place — 511 NX, and 0x8000
// left executable. On OVMF 0x8000 sits in an unsplit 2 MiB leaf, so that leaf IS split and the same
// end state is reached through the pool. Either way exactly one 4 KiB page below 2 MiB stays
// executable, which is the whole page the trampoline occupies (`start_aps` proves `[0x8000, 0x9000)`
// usable before copying into it, and `ap_trampoline_end - ap_trampoline_start` is far under 4 KiB).
// The AP arms EFER.NXE in the trampoline itself (M1's `orl $0x900`) before it turns paging on, so it
// honours these bits from its first paged instruction.
// =================================================================================================

/// Physical-address field of a 2 MiB leaf (bits 51:21). **Not** `PTE_ADDR` — bit 12 of a huge leaf is
/// the PAT selector, not address, and folding it into the base is exactly the GR15 defect.
const PTE_ADDR_2M: u64 = 0x000F_FFFF_FFE0_0000;
/// Physical-address field of a 1 GiB leaf (bits 51:30). Same warning as `PTE_ADDR_2M`.
const PTE_ADDR_1G: u64 = 0x000F_FFFF_C000_0000;

/// Every bit of a leaf entry that a split must carry through UNCHANGED, enumerated rather than
/// inferred: P, W, U/S, PWT, PCD, A, D, G, the two AVL fields and NX. It deliberately EXCLUDES bit 7
/// and bit 12 — the two whose meaning depends on the level — and the address field. Those three are
/// handled by hand at each call site, which is the only way the PAT translation can be made visible.
const WXN_LEAF_CARRY: u64 = PTE_PRESENT
    | PTE_WRITABLE
    | PTE_USER
    | PTE_PWT
    | PTE_PCD
    | (1 << 5)      // Accessed
    | (1 << 6)      // Dirty (leaf-only; ignored on the parents we build)
    | PTE_GLOBAL    // bit 8 — same position at every level
    | (0x7 << 9)    // AVL 11:9
    | (0xF << 59)   // AVL / protection key 62:59
    | PTE_NX;       // bit 63

/// Tables M2 may consume. See the section header for the arithmetic: <= 4 PDs + <= 10 PTs = 14, and
/// 16 is that bound plus slack. 64 KiB of `.bss`.
const WXN_POOL_CAP: usize = 16;

/// The split's frame source. `.bss`, 4 KiB-aligned, identity-mapped — so a page's address IS its
/// physical address (`table_pa`), which is the whole reason M2 can run before the heap exists. Every
/// page taken becomes a live page-table frame for the life of the kernel and is never returned.
static mut WXN_POOL: [PageTable; WXN_POOL_CAP] = [const { PageTable::zeroed() }; WXN_POOL_CAP];

/// One-shot latch. The pool cursor is monotone and its pages become live tables, so a second run
/// would hand out frames that are already wired into the map. M2 is called from `arch::init` on the
/// BSP only; this makes that a checked fact rather than a call-graph argument.
static WXN_M2_DONE: AtomicBool = AtomicBool::new(false);

/// D6 — the flush, and the branch on the wire, extracted from M1's sweep so both milestones take one
/// branch and print one token for it. A parent-entry change invalidates a whole GiB of translations,
/// so `invlpg` at 4 KiB stride is not viable there (262144 instructions per GiB). A CR3 reload flushes
/// everything EXCEPT global entries, and this file records that the firmware's huge identity leaves
/// may carry the Global bit — so if PGE is set we clear-and-restore CR4.PGE, which does evict globals.
/// We never *set* PGE: on a machine where firmware left it clear (the rMBP — Boot V/W, `pge=0`; OVMF
/// likewise) no global entry can exist, a CR3 reload is a complete flush, and setting PGE would be a
/// semantic change masquerading as one.
///
/// Worth stating because it bounds the whole risk of both milestones: every entry write they make is
/// a RESTRICTION (bit 63 set) or a REFINEMENT (one leaf replaced by 512 that reproduce it) with an
/// unchanged output address. A missed flush can therefore only leave the protection vacuous on a
/// stale entry — it can never fault and can never mistranslate.
///
/// `cr4` is the value read before the edit; on return CR4 is bit-identical to it.
fn wxn_flush_tlb(pge: bool, cr4: u64) {
    use x86_64::registers::control::Cr4;
    const CR4_PGE: u64 = 1 << 7;
    if pge {
        // SAFETY: clearing then restoring CR4.PGE flushes the entire TLB including global entries;
        // CR4 ends bit-identical to how we found it. Interrupts are still masked (pre-`sti`).
        unsafe {
            Cr4::write_raw(cr4 & !CR4_PGE);
            Cr4::write_raw(cr4);
        }
    } else {
        // SAFETY: reloading CR3 with its own value is architecturally a full non-global TLB flush;
        // with PGE clear there are no global entries to miss.
        unsafe {
            core::arch::asm!(
                "mov {t}, cr3",
                "mov cr3, {t}",
                t = out(reg) _,
                options(nostack, preserves_flags)
            );
        }
    }
}

/// Bump allocator over `WXN_POOL`. `None` is exhaustion — which the pre-pass has already ruled out
/// before the edit window opens, so a `None` at edit time means the pre-pass and the edit disagree.
struct WxnPool {
    next: usize,
}

impl WxnPool {
    const fn new() -> Self {
        WxnPool { next: 0 }
    }
    fn take(&mut self) -> Option<*mut u64> {
        if self.next >= WXN_POOL_CAP {
            return None;
        }
        // SAFETY: `WXN_POOL` is a `.bss` array of 4 KiB-aligned tables; each index is handed out at
        // most once (the cursor only advances) and M2 runs at most once (`WXN_M2_DONE`).
        let p = unsafe { (&raw mut WXN_POOL[self.next]).cast::<u64>() };
        self.next += 1;
        Some(p)
    }
}

/// The set of virtual addresses M2 leaves EXECUTABLE: the kernel's `PF_X` `PT_LOAD` extent rounded
/// out to page boundaries, plus the single 4 KiB page holding the AP trampoline. Everything else in
/// the spared GiBs gets NX, at the coarsest level that expresses it.
#[derive(Clone, Copy)]
struct WxnKeep {
    x_lo: u64,
    x_hi: u64,
    t_lo: u64,
    t_hi: u64,
}

impl WxnKeep {
    /// Is the page at `va` one of the ones that stays executable?
    #[inline]
    fn holds(&self, va: u64) -> bool {
        (va >= self.x_lo && va < self.x_hi) || (va >= self.t_lo && va < self.t_hi)
    }
    /// Does `[lo, hi)` contain ANY executable page? False ⇒ the whole region can take one NX bit at
    /// its parent, which is how 1023 of the 1024 spared PD entries are retired on metal.
    #[inline]
    fn overlaps(&self, lo: u64, hi: u64) -> bool {
        (lo < self.x_hi && self.x_lo < hi) || (lo < self.t_hi && self.t_lo < hi)
    }
    /// Is EVERY page of `[lo, hi)` executable? True ⇒ leave the leaf exactly as it is; no split, no
    /// table, no write. (A 2 MiB-or-larger `.text` reaches this; today's ~1 MiB `.text` does not.)
    #[inline]
    fn covers(&self, lo: u64, hi: u64) -> bool {
        (lo >= self.x_lo && hi <= self.x_hi) || (lo >= self.t_lo && hi <= self.t_hi)
    }
}

/// Runtime `[start, end)` of the union of the image's NON-WRITABLE `PT_LOAD` segments, rounded OUT to
/// 4 KiB boundaries, plus the count of segments unioned. `None` if the ELF header is unreadable or
/// every LOAD is writable, in which case M2 refuses rather than guess.
///
/// **`PF_W`, not `PF_X`** — see the section header. `switch_context` is live ring-0 code that the
/// linker put in `.rodata` (a `PF_X=0` LOAD) because of a leaked `.section .rodata` in `smp.rs`, and
/// a `PF_X`-based extent page-faults on the first task switch. The read-only LOADs are the ones that
/// can legitimately hold executed code; the writable ones never can.
///
/// Rounding OUT is the fail-safe direction: sub-page granularity does not exist, so a page shared
/// between a read-only and a writable segment keeps X. Whether any page IS shared on a given link is
/// a `rust-lld` layout fact, not a property of this kernel, so it is MEASURED rather than asserted
/// here: M3b subtracts `wxn_wdata_span()` from this extent page by page and prints the count as
/// `wskip=` on its own line. (An earlier version of this comment claimed "on this image exactly one
/// page is shared — `.text`'s tail and `.data.rel.ro`'s head". That was stale: on every link measured
/// since, the two segments land on distinct pages and `wskip=0`. The number on the wire is the
/// record; a comment is not. — review-m3b-draft.md C10.)
///
/// Same PIE reasoning as `wxn_image_bounds`: `min_vaddr == 0`, so a segment's runtime address is
/// `ehdr + p_vaddr` exactly.
fn wxn_exec_extent() -> Option<(u64, u64, u32)> {
    wxn_load_union(false)
}

/// The page-rounded-OUT union of the image's `PT_LOAD` segments whose `PF_W` bit equals
/// `want_writable`, plus the count of segments unioned. `wxn_exec_extent` is the `false` arm (the
/// executable extent M2 spares); M3b's `wxn_wdata_span` is the `true` arm (the pages the image itself
/// declares mutable, which M3b must never make read-only).
///
/// ONE phdr WALK, TWO QUESTIONS — deliberately, per the CFU-2 precedent: M2's keep set and M3b's
/// exclusion set have to come from the same header read by the same code, or a disagreement between
/// two hand-rolled walks becomes a page that is executable under one rule and writable under the
/// other. Splitting the predicate out is the whole change; the `false` arm is behaviour-identical to
/// the walk M2 has run since `e8b11513`.
fn wxn_load_union(want_writable: bool) -> Option<(u64, u64, u32)> {
    const PT_LOAD: u32 = 1;
    const PF_W: u32 = 2;
    let ehdr = &raw const __ehdr_start as u64;
    // SAFETY: reads only, through a pointer the linker resolved to the loaded image base. The magic /
    // class / phentsize checks are what make the phdr reads below safe to perform at all.
    let ok = unsafe {
        let p = ehdr as *const u8;
        core::ptr::read_unaligned(p.cast::<u32>()) == 0x464C_457F // "\x7fELF"
            && *p.add(4) == 2 // ELFCLASS64
            && core::ptr::read_unaligned(p.add(54).cast::<u16>()) == 56 // e_phentsize
    };
    if !ok {
        return None;
    }
    let (phoff, phnum) = unsafe {
        let p = ehdr as *const u8;
        (
            core::ptr::read_unaligned(p.add(32).cast::<u64>()),
            core::ptr::read_unaligned(p.add(56).cast::<u16>()) as usize,
        )
    };
    let (mut lo, mut hi, mut segs) = (u64::MAX, 0u64, 0u32);
    for i in 0..phnum.min(64) {
        // SAFETY: `phoff`/`phnum` come from the header validated above; each phdr is 56 bytes inside
        // the first PT_LOAD, which the bootloader copied into RAM along with the header.
        let (ptype, flags, vaddr, memsz) = unsafe {
            let ph = (ehdr + phoff + (i as u64) * 56) as *const u8;
            (
                core::ptr::read_unaligned(ph.cast::<u32>()),
                core::ptr::read_unaligned(ph.add(4).cast::<u32>()),
                core::ptr::read_unaligned(ph.add(16).cast::<u64>()),
                core::ptr::read_unaligned(ph.add(40).cast::<u64>()),
            )
        };
        if ptype != PT_LOAD || (flags & PF_W != 0) != want_writable || memsz == 0 {
            continue;
        }
        segs += 1;
        lo = lo.min((ehdr + vaddr) & !(PAGE_4K - 1));
        hi = hi.max((ehdr + vaddr + memsz + PAGE_4K - 1) & !(PAGE_4K - 1));
    }
    if hi <= lo { None } else { Some((lo, hi, segs)) }
}

/// Build the 512 4 KiB PTEs that reproduce the 2 MiB leaf `e` exactly — same physical pages, same
/// memory type, same W/U/G/A/D — and mark NX every one whose VA is outside `keep`. The table is
/// written with its FINAL values; the caller installs it with a single store afterwards.
///
/// The PAT translation lives here and nowhere else: a 2 MiB leaf selects its PAT index with bit 12, a
/// 4 KiB PTE with bit 7. `WXN_LEAF_CARRY` excludes both, so neither can survive by accident.
///
/// Returns `(entries minted non-executable, entries left executable)`.
unsafe fn wxn_fill_pt_from_2m(pt: *mut u64, e: u64, va_base: u64, keep: &WxnKeep) -> (u32, u32) {
    let pa = e & PTE_ADDR_2M;
    let carry = (e & WXN_LEAF_CARRY) | if e & PTE_PAT_HUGE != 0 { PTE_PAT_4K } else { 0 };
    let (mut nx, mut kx) = (0u32, 0u32);
    for i in 0..512u64 {
        let mut v = (pa + (i << 12)) | carry;
        if !keep.holds(va_base + (i << 12)) {
            v |= PTE_NX;
        }
        if v & PTE_NX != 0 {
            nx += 1;
        } else {
            kx += 1;
        }
        // SAFETY: `pt` is a fresh, exclusively-owned 4 KiB pool page; `i < 512`.
        unsafe { core::ptr::write_volatile(pt.add(i as usize), v) };
    }
    (nx, kx)
}

/// Build the 512 2 MiB leaves that reproduce the 1 GiB leaf `e` exactly. The PAT bit does NOT move
/// here (bit 12 at both levels) and `PTE_HUGE` stays set — the demotion changes granularity only.
/// Permissions are refined afterwards by the PD loop, exactly as if the firmware had mapped it this
/// way. Neither known platform reaches this path (`l1=0` on both); it exists so a firmware that DOES
/// use 1 GiB leaves for the image's GiB is handled rather than silently left RWX.
unsafe fn wxn_fill_pd_from_1g(pd: *mut u64, e: u64) {
    let pa = e & PTE_ADDR_1G;
    let carry = (e & WXN_LEAF_CARRY) | (e & PTE_PAT_HUGE) | PTE_HUGE;
    for i in 0..512u64 {
        // SAFETY: `pd` is a fresh, exclusively-owned 4 KiB pool page; `i < 512`.
        unsafe { core::ptr::write_volatile(pd.add(i as usize), (pa + (i << 21)) | carry) };
    }
}

/// The non-leaf entry that replaces leaf `e` and points at the freshly built table at `child_pa`.
///
/// Bits are chosen, not copied: at a non-leaf, bit 7 is PS (must be 0), bit 12 is address, bit 6 is
/// ignored, and bit 8 is ignored — so a blind copy would be wrong in four places. What IS carried is
/// exactly the set whose meaning is the same at both levels and whose fold decides the effective
/// permission: W, U/S and NX (hardware ANDs W and U/S down the path and ORs NX). Because the 512
/// children carry the same three bits from the same `e`, the effective permission of every byte in
/// the region is bit-identical to what `e` gave it. PWT/PCD are carried too — at a parent they type
/// the page-table WALK, not the pages, so this is cosmetic, but carrying them keeps a UC region's
/// walk UC as the firmware had it.
#[inline]
fn wxn_parent_from_leaf(e: u64, child_pa: u64) -> u64 {
    (child_pa & PTE_ADDR)
        | PTE_PRESENT
        | (e & (PTE_WRITABLE | PTE_USER | PTE_PWT | PTE_PCD | PTE_NX))
}

/// **The splitter.** Called once, at the end of `wxn_pdpt_sweep`, with the spare set M1 computed.
///
/// Refuses (loudly, writing nothing) if the image declares no executable segment, if the derived
/// extent is not inside the image bounds M1 already validated, or if the split would need more tables
/// than the static pool holds. Panics if its own address is not inside the extent it is about to
/// spare, if a leaf it is about to split is not identity-mapped, if that leaf is the framebuffer's,
/// or if the framebuffer leaf changes in anything but `PTE_NX` across the window.
fn wxn_split_stage(spare: &WxnSpare, img_lo: u64, img_hi: u64, pge: bool, cr4: u64) {
    if WXN_M2_DONE.swap(true, Ordering::AcqRel) {
        serial_println!(":: WXN-M2: -> REFUSED (already run; the pool's pages are live tables) ::");
        return;
    }

    // Leg 1 — fail closed on the executable extent. Without it M2 cannot know which pages must keep
    // X, and a guess makes the running kernel non-executable.
    let Some((x_lo, x_hi, x_segs)) = wxn_exec_extent() else {
        serial_println!(
            ":: WXN-M2: ehdr=0x{:X} xseg=none -> REFUSED (no read-only PT_LOAD; no entry written) ::",
            &raw const __ehdr_start as u64
        );
        return;
    };
    // Leg 1b — the extent must live inside the bounds M1 already proved contain this code. A phdr
    // walk that produced an extent outside the image is a walk that read the wrong header.
    if x_lo < img_lo || x_hi > img_hi {
        serial_println!(
            ":: WXN-M2: xseg=[0x{:X},0x{:X}) img=[0x{:X},0x{:X}) -> REFUSED (executable extent \
             outside the image; no entry written) ::",
            x_lo, x_hi, img_lo, img_hi
        );
        return;
    }
    let tramp = super::smp::TRAMPOLINE_ADDR as u64;
    let keep = WxnKeep { x_lo, x_hi, t_lo: tramp, t_hi: tramp + PAGE_4K };

    // Leg 2 — THE load-bearing assert, the M2 twin of M1's Leg 3 and for the same reason: this
    // function is itself a `.text` address, so it must be inside the extent M2 is about to spare. If
    // the `PF_X` walk latched onto the wrong segment, this fires here — before a single entry is
    // written — instead of as a dead machine one instruction after the flush.
    let here = wxn_split_stage as *const () as u64;
    assert!(
        keep.holds(here),
        "WXN-M2: the derived executable extent [{:#x},{:#x}) does not contain this function \
         ({:#x}) — the read-only-LOAD phdr walk found the wrong segment",
        x_lo, x_hi, here
    );

    // -----------------------------------------------------------------------------------------
    // Pre-pass: count the tables the edit will consume, WITHOUT writing anything. This is what
    // makes pool exhaustion a refusal rather than a half-split map — the decision is taken while
    // the map is still untouched.
    // -----------------------------------------------------------------------------------------
    let (mut need_pd, mut need_pt) = (0usize, 0usize);
    for s in 0..spare.n {
        let gva = spare.gib[s] << 30;
        // SAFETY: every page-table frame is identity-mapped and directly readable at ring 0. Reads
        // only — this pass must not perturb the map it is measuring.
        unsafe {
            let e4 = *cr3_table().add(pml4_index(gva));
            if e4 & PTE_PRESENT == 0 || e4 & PTE_USER != 0 {
                continue;
            }
            let e3 = *((e4 & PTE_ADDR) as *const u64).add(pdpt_index(gva));
            if e3 & PTE_PRESENT == 0 || e3 & PTE_USER != 0 {
                continue;
            }
            if !keep.overlaps(gva, gva + (1 << 30)) {
                continue; // one NX bit on the PDPT entry retires the whole GiB — no table needed
            }
            let pd = if e3 & PTE_HUGE != 0 {
                need_pd += 1;
                core::ptr::null::<u64>() // demoted: all 512 children will be 2 MiB leaves
            } else {
                (e3 & PTE_ADDR) as *const u64
            };
            for k in 0..512u64 {
                let (rlo, rhi) = (gva + (k << 21), gva + (k << 21) + (1 << 21));
                if !keep.overlaps(rlo, rhi) || keep.covers(rlo, rhi) {
                    continue;
                }
                if pd.is_null() {
                    need_pt += 1; // a demoted GiB's children are all 2 MiB leaves
                    continue;
                }
                let e2 = *pd.add(k as usize);
                if e2 & PTE_PRESENT == 0 || e2 & PTE_USER != 0 {
                    continue;
                }
                if e2 & PTE_HUGE != 0 {
                    need_pt += 1; // already 4 KiB-granular ⇒ edited in place, no table needed
                }
            }
        }
    }
    let need = need_pd + need_pt;
    if need > WXN_POOL_CAP {
        serial_println!(
            ":: WXN-M2: xseg=[0x{:X},0x{:X}) need_pd={} need_pt={} pool_cap={} -> REFUSED (static \
             pool exhausted; no entry written) ::",
            x_lo, x_hi, need_pd, need_pt, WXN_POOL_CAP
        );
        return;
    }

    // R2 / GR15, M2's own belt. M1's interlock has already been cashed above and bracketed only M1;
    // this one brackets only M2. M2 CAN legitimately change the fb leaf — if the panel happened to
    // live inside a spared GiB its leaf takes an NX bit like any other — so `PTE_NX` is the one
    // permitted delta and anything else stops the boot. Splitting it is not permitted at all: the
    // `leaf_is_fb_wc` refusal below fires first.
    let fb = crate::video::WRITER.try_lock().map(|w| w.base() as u64).unwrap_or(0);
    let fb_before = if fb != 0 { wx_probe_leaf(fb).map(|(e, l, _)| (e, l)) } else { None };

    // -----------------------------------------------------------------------------------------
    // The edit.
    // -----------------------------------------------------------------------------------------
    let mut pool = WxnPool::new();
    let (mut demote_1g, mut split_2m) = (0u32, 0u32);
    let (mut nx_pdpt, mut nx_2m, mut nx_pt, mut nx_4k) = (0u32, 0u32, 0u32, 0u32);
    let (mut keep_x, mut already_nx, mut skip_user) = (0u32, 0u32, 0u32);

    with_page_tables_writable(|| {
        // SAFETY: every page-table frame is identity-mapped and directly addressable at ring 0, and
        // CR0.WP is clear for this window so the firmware's read-only table pages accept the stores.
        // Interrupts are masked by the wrapper. Every write is either `e | PTE_NX` (a permission-only
        // RMW that moves bit 63 and nothing else) or a table install whose 512 children were fully
        // populated first, so no intermediate state is ever reachable.
        unsafe {
            for s in 0..spare.n {
                let gva = spare.gib[s] << 30;
                let e4 = core::ptr::read_volatile(cr3_table().add(pml4_index(gva)));
                if e4 & PTE_PRESENT == 0 {
                    continue;
                }
                if e4 & PTE_USER != 0 {
                    skip_user += 1;
                    continue;
                }
                let p3 = ((e4 & PTE_ADDR) as *mut u64).add(pdpt_index(gva));
                let e3 = core::ptr::read_volatile(p3);
                if e3 & PTE_PRESENT == 0 {
                    continue;
                }
                if e3 & PTE_USER != 0 {
                    skip_user += 1;
                    continue;
                }

                // A spared GiB with nothing executable in it — the image's SECOND GiB when `.text`
                // lands entirely in the first, say. One write retires 512 (or 262144) leaves.
                if !keep.overlaps(gva, gva + (1 << 30)) {
                    if e3 & PTE_NX == 0 {
                        core::ptr::write_volatile(p3, e3 | PTE_NX);
                        nx_pdpt += 1;
                    } else {
                        already_nx += 1;
                    }
                    continue;
                }

                let pd: *mut u64 = if e3 & PTE_HUGE != 0 {
                    // A 1 GiB LEAF holding executable code. Demote it to 512 x 2 MiB so the PD loop
                    // below can work at 2 MiB granularity like everywhere else.
                    assert!(
                        !leaf_is_fb_wc(e3, gva, 1 << 30, PTE_PAT_HUGE),
                        "WXN-M2: refusing to split the framebuffer's WC 1 GiB leaf at {:#x} \
                         (e=0x{:016X}) — this is the GR15 hazard and the splitter will not take it",
                        gva, e3
                    );
                    assert!(
                        e3 & PTE_ADDR_1G == gva,
                        "WXN-M2: the spared 1 GiB leaf at VA {:#x} maps PA {:#x} — the map is not \
                         identity there, so a VA-derived keep set cannot be applied to it",
                        gva,
                        e3 & PTE_ADDR_1G
                    );
                    let nt = pool.take().unwrap_or_else(|| {
                        panic!("WXN-M2: pool exhausted mid-edit — the pre-pass under-counted")
                    });
                    wxn_fill_pd_from_1g(nt, e3);
                    core::ptr::write_volatile(p3, wxn_parent_from_leaf(e3, table_pa(nt)));
                    for i in 0..512u64 {
                        invlpg(gva + (i << 21)); // 2 MiB stride: 4 KiB would be 262144 instructions
                    }
                    demote_1g += 1;
                    nt
                } else {
                    (e3 & PTE_ADDR) as *mut u64
                };

                for k in 0..512usize {
                    let p2 = pd.add(k);
                    let e2 = core::ptr::read_volatile(p2);
                    if e2 & PTE_PRESENT == 0 {
                        continue;
                    }
                    if e2 & PTE_USER != 0 {
                        skip_user += 1;
                        continue;
                    }
                    let rlo = gva + ((k as u64) << 21);
                    let rhi = rlo + (1 << 21);

                    // (a) nothing executable in this 2 MiB — one NX bit at the PD level, whether the
                    // entry is a leaf or a table pointer. This is where 1023 of metal's 1024 spared
                    // PD entries go.
                    if !keep.overlaps(rlo, rhi) {
                        if e2 & PTE_NX == 0 {
                            core::ptr::write_volatile(p2, e2 | PTE_NX);
                            if e2 & PTE_HUGE != 0 {
                                nx_2m += 1;
                            } else {
                                nx_pt += 1;
                            }
                        } else {
                            already_nx += 1;
                        }
                        continue;
                    }

                    if e2 & PTE_HUGE != 0 {
                        // (b) a 2 MiB leaf entirely inside the keep set — leave it exactly as it is.
                        if keep.covers(rlo, rhi) {
                            keep_x += 1;
                            continue;
                        }
                        // (c) a 2 MiB leaf STRADDLING the keep set — the only case that needs a table.
                        assert!(
                            !leaf_is_fb_wc(e2, rlo, 1 << 21, PTE_PAT_HUGE),
                            "WXN-M2: refusing to split the framebuffer's WC 2 MiB leaf at {:#x} \
                             (e=0x{:016X}) — this is the GR15 hazard and the splitter will not take it",
                            rlo, e2
                        );
                        assert!(
                            e2 & PTE_ADDR_2M == rlo,
                            "WXN-M2: the 2 MiB leaf at VA {:#x} maps PA {:#x} — the map is not \
                             identity there, so a VA-derived keep set cannot be applied to it",
                            rlo,
                            e2 & PTE_ADDR_2M
                        );
                        let nt = pool.take().unwrap_or_else(|| {
                            panic!("WXN-M2: pool exhausted mid-edit — the pre-pass under-counted")
                        });
                        let (nx, kx) = wxn_fill_pt_from_2m(nt, e2, rlo, &keep);
                        nx_4k += nx;
                        keep_x += kx;
                        core::ptr::write_volatile(p2, wxn_parent_from_leaf(e2, table_pa(nt)));
                        for i in 0..512u64 {
                            invlpg(rlo + (i << 12));
                        }
                        split_2m += 1;
                    } else {
                        // (d) already 4 KiB-granular (Apple EFI's low GiB, and the firmware's other
                        // PTs) — refine in place, no table, no pool page.
                        let pt = (e2 & PTE_ADDR) as *mut u64;
                        for l in 0..512usize {
                            let p1 = pt.add(l);
                            let e1 = core::ptr::read_volatile(p1);
                            if e1 & PTE_PRESENT == 0 {
                                continue;
                            }
                            if e1 & PTE_USER != 0 {
                                skip_user += 1;
                                continue;
                            }
                            let va = rlo + ((l as u64) << 12);
                            if keep.holds(va) {
                                if e1 & PTE_NX == 0 {
                                    keep_x += 1;
                                }
                                continue;
                            }
                            if e1 & PTE_NX != 0 {
                                already_nx += 1;
                                continue;
                            }
                            core::ptr::write_volatile(p1, e1 | PTE_NX);
                            invlpg(va);
                            nx_4k += 1;
                        }
                    }
                }
            }
        }
    });

    wxn_flush_tlb(pge, cr4);

    // The GR15 belt, cashed for M2's own window.
    let fb_delta = match (fb_before, if fb != 0 { wx_probe_leaf(fb) } else { None }) {
        (Some((e_before, l_before)), Some((e_after, l_after, _))) => {
            assert!(
                l_after == l_before,
                "WXN-M2: the framebuffer walk terminated at a DIFFERENT LEVEL across the split — \
                 before=0x{:016X}/lvl{} after=0x{:016X}/lvl{}. M2 splits only leaves that hold \
                 executable code and refuses fb leaves outright, so the fb's level cannot move.",
                e_before, l_before, e_after, l_after
            );
            let delta = e_after ^ e_before;
            assert!(
                delta == 0 || delta == PTE_NX,
                "WXN-M2: the framebuffer LEAF changed in bits other than PTE_NX — \
                 before=0x{:016X} after=0x{:016X} delta=0x{:016X} lvl={}. This is the GR15 defect \
                 (a dropped PAT/PCD/PWT bit silently re-UCs the panel, 8.7-9.1x on the blit path) \
                 and it is invisible to every permission instrument, so the boot stops here.",
                e_before, e_after, delta, l_after
            );
            delta
        }
        (Some((e_before, _)), None) => panic!(
            "WXN-M2: the framebuffer mapping at {:#x} is gone after the split — it was present \
             (leaf 0x{:016X}) before it",
            fb, e_before
        ),
        _ => 0,
    };

    // The verdict. Same shape and same reason as M1's: a stage that wrote NOTHING must not print a
    // line that reads normal. `spare.n > 0` is guaranteed by M1 (the trampoline GiB is always
    // inserted), so zero writes means every descent bailed on a `skip_` branch.
    let wrote = demote_1g + split_2m + nx_pdpt + nx_2m + nx_pt + nx_4k;
    let verdict = if wrote == 0 { "-> VACUOUS" } else { "-> SPLIT" };
    serial_println!(
        ":: WXN-M2: xseg=[0x{:X},0x{:X}) xsegs={} xpages={} tramp=0x{:X} spare_n={} demote_1g={} split_2m={} \
         pool_used={}/{} nx_pdpt={} nx_2m={} nx_pt={} nx_4k={} keep_x={} already_nx={} skip_user={} \
         fb=0x{:X} fb_delta=0x{:X} pge={} flush={} {} ::",
        x_lo,
        x_hi,
        x_segs,
        (x_hi - x_lo) / PAGE_4K,
        tramp,
        spare.n,
        demote_1g,
        split_2m,
        pool.next,
        WXN_POOL_CAP,
        nx_pdpt,
        nx_2m,
        nx_pt,
        nx_4k,
        keep_x,
        already_nx,
        skip_user,
        fb,
        fb_delta,
        pge as u8,
        if pge { "pge-toggle" } else { "cr3-reload" },
        verdict,
    );
    // M2's self-prediction, the twin of M1's `residue_leaves`. `keep_x` is counted during the edit;
    // `kern_WX` on the WXAUDIT line one screen down is counted by a completely separate walk of the
    // same tables. They must agree (less any executable leaf the firmware left read-only, of which
    // there are none in `.text` — the kernel writes its own `.data`), and `keep_x` must in turn equal
    // `xpages + 1` above, which is derived from the ELF header alone. Three independent derivations
    // of one number, on two adjacent lines.
}

// =================================================================================================
// WXN-x86 M3b — clearing W from the kernel's executable pages. THE FIRST READ-ONLY KERNEL PAGE.
// -------------------------------------------------------------------------------------------------
// WHAT M1/M2/M3a LEFT. M1 put NX on 126 of 128 GiB; M2 shrank the executable residue to exactly the
// image's non-writable `PT_LOAD` union plus the AP trampoline page; M3a armed `CR0.WP` on every core
// so that a leaf's W bit binds a CPL-0 store at all (metal Boot AB: `wp=8 wp_mask=0xFF -> PASS`,
// `cr0=0x0000000080010013`). Every page in that residue is still WRITABLE — every `WXPROBE map:
// at=ktext` in the record reads `w=1 fw=1 fx=1`. The map so far constrains what may EXECUTE; it does
// not constrain what may be WRITTEN. M3b closes the W column.
//
// THE POPULATION, and it is not M2's keep set:
//
//   * THE TRAMPOLINE PAGE STAYS WRITABLE — a lifecycle fact, cited not assumed. `arch::init` (where
//     M1, M2 and this stage run) executes at `main.rs -> lib.rs`, and `smp::start_aps()` strictly
//     AFTER it. `start_aps` writes the trampoline page wholesale once
//     (`smp.rs`, `copy_nonoverlapping(ap_trampoline_start, TRAMPOLINE_ADDR, len)`) and then once PER
//     AP (`patch_param(ap_param_stack/ap_param_index)`, inside the per-AP loop, because the handoff
//     slot is shared and reused for every core). With `CR0.WP` armed a read-only `0x8000` turns the
//     first of those stores into a fatal CPL-0 `#PF` before a single AP starts. So M3b leaves it
//     alone and SAYS SO on the wire (`tramp_w=`). **`kern_WX` floors at 1, not 0, and the residue IS
//     the trampoline.** Anyone predicting 0 is predicting a boot with no APs. Making it RX needs a
//     lifecycle — clear W after the LAST AP reports online, inside `start_aps` — which is a separate
//     change in a separate file and is named M3c here rather than smuggled in.
//
//   * THE EXTENT IS ROUNDED *IN*, NOT OUT. `wxn_exec_extent` rounds OUT because for X the fail-safe
//     direction is "keep it executable". For W it is the OPPOSITE: a page any `PF_W` LOAD touches,
//     even by one byte, must keep W or a legitimate write to `.data.rel.ro`/`.got`/`.data` becomes a
//     `#PF`. M3b therefore subtracts the page-rounded-OUT union of the WRITABLE LOADs
//     (`wxn_wdata_span`) per page and counts it as `wskip=`. On the links measured to date the two do
//     not overlap — but that is a `rust-lld` default, not a property of this kernel, so it is
//     SUBTRACTED AT RUNTIME. `wskip=0` on the wire is evidence that it held on the boot you are
//     reading; it is not a claim that it always will. A non-zero `wskip` is a W^X HOLE (a page in the
//     exec extent that keeps W *and* keeps X) and a finding to chase, not a benign count.
//
// WHAT MAY WRITE INTO THE RANGE — from artefacts, not from confidence:
//   * RELOCATIONS. The image is PIE and carries ~1.5k entries, all `R_X86_64_RELATIVE`, every target
//     inside LOAD 03 (`RW`) and ZERO below it — i.e. none inside the extent. They are applied by the
//     BOOTLOADER before it ever jumps to `_start`, so they are complete before the kernel's first
//     instruction.
//   * SELF-PATCHING. None: no `&raw const ... as *mut`, no `addr_of!(..) as *mut`, no
//     `.as_ptr() as *mut`, no `.cast_mut()` anywhere under `crates/kernel/src`. The `.text`/`.rodata`
//     addresses the kernel does take are READ sources — `ap_trampoline_start` (a memcpy source), the
//     `unaos_user_*_blob_start` symbols (copy sources for the ring-3 window) and `__ehdr_start`.
//   * NO MUTABLE DATA IN `.text`. `readelf -sW` finds zero `OBJECT` symbols in `.text`; a
//     disassembly-wide sweep of every RIP-relative STORE-shaped instruction in the image resolves
//     zero targets below the extent's top.
//   * `.section` LEAKS. Closed at the source (`smp.rs`'s `.pushsection`/`.popsection`, `eb6cd7c2`)
//     and held closed by a post-link gate (`gate_x86_rodata_no_code`, which fails the x86 kernel link
//     on any GLOBAL `FUNC`/`NOTYPE` symbol in `.rodata` other than the trampoline's own labels). It
//     says nothing about a page a `PF_W` segment shares — that is `wskip`'s job, not the gate's.
//   * STACKS / DESCRIPTOR TABLES. AP stacks, IST stacks, percpu and the GDT are all `.bss` (LOAD 04
//     `RW`) — which also closes a class worth naming: the CPU's own GDT ACCESSED-BIT write would
//     fault on a read-only descriptor table, and the GDT is not in the extent.
//   * HEAP. Does not exist yet when M3b runs, and M3b allocates nothing.
//   * WHAT IS GENUINELY UNCOVERED, and it should be said: a store through a pointer whose value comes
//     from firmware/`BootInfo`/a device BAR, and DMA. DMA is not paging-checked (no IOMMU), so M3b
//     adds NO brick risk there — a DMA scribble into `.text` stays exactly as silent post-M3b as it
//     is today.
//
// ORDERING, and the property it buys for free. M3b runs where M1 and M2 run — inside `arch::init`,
// interrupts masked, before `sti`, immediately after `wxn_split_stage` (it needs the 4 KiB
// granularity M2 created) and well before `start_aps`. Because NO AP HAS BEEN STARTED YET, no core
// other than the BSP can ever have cached a writable translation of these pages — the same argument
// U1b B4 makes for the ring-3 code page, and the reason this needs no cross-core shootdown. The slot
// page tables are not a second copy: `build_slot` COPIES THE KERNEL PML4 ENTRIES, which point at the
// same lower tables, so a leaf edited here is the same leaf every process CR3 walks.
//
// WHAT BINDS IT, per core. `CR0.WP` is per-core state. The BSP armed it in `syscall::init` before
// `wxn_pdpt_sweep`, so the RO binds on the BSP from the instant M3b writes it. Each AP arms its own
// in `syscall::init` inside `ap_entry`; between its first paged instruction and that call an AP runs
// with WP=0. Nothing in that window writes `.text` — it is APIC/GDT/percpu setup — but the honest
// statement is that the RO regime is complete only once `wp_mask` is full, and the witness for that
// is the EXISTING `WXAUDIT-NXE: ... wp=8 wp_mask=0xFF -> PASS` line, not a new one.
//
// THE ARM, AND WHY IT IS NOT A CONVENIENCE. This is the change most able to brick a boot since WXN
// began: M1 and M2 only ever restricted EXECUTION of pages nothing executes, and a missed flush there
// "can only leave the protection vacuous ... it can never fault". M3b restricts WRITES on the pages
// the kernel is running from. One missed writer is a fatal CPL-0 `#PF`. So the WRITE is behind
// `feature = "wxnro"` (`UNAOS_WXNRO=1`) and the WALK is not: DISARMED — the default, and every
// existing build path — this stage takes the identical census, writes NOTHING, and prints `would=`
// where an armed boot prints `cleared=`. Recovery from a bricked armed boot is one media rebuild with
// the knob unset, not a revert.
//
// `cfg!(feature = ...)`, NOT `#[cfg]`: the store is COMPILED and type-checked in every leg of
// `./arroyo check` whichever way the knob is set, so no cfg-matrix leg can be green on code the armed
// build never saw. (GR19 paid for that lesson with a gate blind to mixed knobs.)
//
// CENSUS FIRST, THEN WRITE — and it is why this needs only ONE armed boot. M2, the strictly LESS
// dangerous stage, takes its full census before writing anything. M3b does the same: a read-only
// pre-pass walks the extent, counts `would`/`wskip`/`huge_*`/`absent`/`already_ro`, and PRINTS
// `:: WXN-M3B-PRE: ... -> CENSUS ::` BEFORE the write window opens. Three things follow. (1) An armed
// boot is self-predicting — it carries its own prediction and its own result, and the closure between
// them is checkable inside one capture. (2) A boot that dies anywhere in the write window still
// carries its census, which is the case a separate dry-run flight could never have helped with.
// (3) A DRY RUN CANNOT PREDICT THE BRICK MODE ANYWAY: every finding a disarmed boot can make
// (`wskip`, `huge_leaves`, `absent`, a `would` that misses expectation, the extent assert, the
// `wseg=none` refusal) either degrades the armed boot gracefully or fires IDENTICALLY in both arms.
// The one condition that can brick — an unknown runtime writer inside the extent — writes nothing
// during a dry run and is invisible to it by construction. A dry run reduces CENSUS risk, not BRICK
// risk, and must not be sold as the latter.
//
// FAIL-CLOSED, AND IT CAN SAY NO. The pre-pass is what makes refusal possible at all, because the
// decision is taken while the map is still untouched. M3b writes only if the extent is EXACTLY what
// it models: every page a present 4 KiB leaf, and every non-`wskip` page currently writable. A huge
// leaf, an absent page or an `already_ro` page means the map is not what this stage modelled, so it
// REFUSES with the census on the wire and an UNMODIFIED map rather than half-applying a rule it does
// not understand. And because that refusal guarantees `already_ro == 0`, the cleared population is
// exactly the `would` population — which is what makes the ROLLBACK below exact.
//
// THE VERIFY PASS IS A SECOND, INDEPENDENT COUNT — taken after the flush, through the FOLD rather
// than the leaf bit. `live_leaf` is the walker `translate`, WXAUDIT and CFU-2's `user_range_leaf_ok`
// are all built on, so what is checked is the writability the HARDWARE computes for the page, not the
// one bit this stage happened to touch: a parent that still folds W=1 over a leaf that folds W=0 is
// not a state the fold can hide. If it does not close, M3b RESTORES W on exactly the pages it cleared
// and refuses — the map ends where it began, which is the state this kernel has booted in every day
// of its life. Never half-applied.
//
// WHAT IS *NOT* PROVEN BY ANY OF THIS. `verify_w=0` and `WXPROBE map: at=ktext ... w=0` are both
// PAGE-TABLE readbacks — they prove the bits, not the hardware's honouring of them. The only true
// falsifier would be a CPL-0 store into the extent that is EXPECTED to fault and recovered from, a
// ring-0 twin of the `u1b-code-write` fixture, which this kernel has no `#PF`-resume path for. That
// gap is named here rather than papered over, and it is the natural M3d.
//
// THE FALSIFIERS ARE INTRA-BOOT, on M3b's own two lines — deliberately NOT a cross-boot comparison
// against a disarmed flight, because `cfg!(feature = "wxnro")` folds to a constant and the two arms
// are DIFFERENT BINARIES with different `.text` sizes. That is not a theoretical worry: the two QEMU
// arms of this very commit link to `xpages=285` disarmed and `xpages=286` armed — a one-page
// difference on a perfectly healthy pair, which is exactly the shape that would have failed a
// cross-boot `cleared == the other boot's would` check. The sound closures are all INSIDE one boot:
//
//     would + already_ro + wskip + absent + huge_pages == xpages   — the census line closes on itself
//     cleared == would                                   (armed)   — prediction vs result, ONE binary
//     verify_w == expect_w                                         — the fold agrees with the leaf bit
//     kern_WX == keep_x - already_ro - cleared                     — replaces `kern_WX == keep_x`
//
// NOTE that `would` is a CENSUS field and is populated in BOTH arms — it is what this stage WOULD
// clear, counted before any write, not "the clears that did not happen". So it does NOT appear in the
// page closure alongside `cleared`; the two are a prediction and its result, and `cleared == would`
// is the check that binds them. Disarmed, `cleared == 0` and the audit identity degenerates to the
// pre-M3b `kern_WX == keep_x`, which is what makes a disarmed boot a true no-op on the analyzer.
// =================================================================================================

/// One-shot latch. M3b is idempotent in principle (clearing a clear bit is a no-op) but it is called
/// from `arch::init` on the BSP only, and a second run would mean something re-entered the sweep —
/// a fact worth printing rather than absorbing.
static WXN_M3B_DONE: AtomicBool = AtomicBool::new(false);

/// The page-rounded-OUT union of the image's WRITABLE `PT_LOAD` segments — the pages M3b must leave
/// writable no matter what the executable extent says, because the ELF itself declares them mutable.
/// `None` means the image has no writable LOAD, which for this kernel means the header did not parse;
/// M3b treats that as a refusal, not as an empty exclusion set.
#[inline]
fn wxn_wdata_span() -> Option<(u64, u64)> {
    wxn_load_union(true).map(|(lo, hi, _)| (lo, hi))
}

/// **The W-clear.** Called once, immediately after `wxn_split_stage`, with the same flush parameters
/// M1 and M2 used. See the section header for the population, the writer inventory and the staging.
///
/// Refuses — loudly, with the census on the wire and NOTHING written — if it has already run, if the
/// executable extent is unreadable, if the image declares no writable LOAD, if the extent is not the
/// all-4-KiB-present-writable map this stage models, or if the post-write verify does not close (in
/// which case the pages it cleared are restored first). Panics only if its own address is outside the
/// extent it is about to make read-only (the M3b twin of M1's Leg 3 and M2's Leg 2, evaluated before
/// any write) or if the framebuffer leaf moves across its window.
fn wxn_ro_stage(pge: bool, cr4: u64) {
    if WXN_M3B_DONE.swap(true, Ordering::AcqRel) {
        serial_println!(":: WXN-M3B: -> REFUSED (already run; nothing written) ::");
        return;
    }

    // Leg 1 — the same extent M2 spared, from the same walk. Not recomputed differently: if these two
    // ever disagreed, a page would be executable under one rule and read-only under the other.
    let Some((x_lo, x_hi, _)) = wxn_exec_extent() else {
        serial_println!(
            ":: WXN-M3B: ehdr=0x{:X} xseg=none -> REFUSED (no read-only PT_LOAD; no entry written) ::",
            &raw const __ehdr_start as u64
        );
        return;
    };
    // Leg 2 — the writable span. Its ABSENCE is a refusal and not an empty set: an image with no
    // `PF_W` LOAD is an image whose header this walk did not understand, and guessing "then nothing
    // is writable" is exactly the shape of assumption this milestone exists to stop making.
    let Some((w_lo, w_hi)) = wxn_wdata_span() else {
        serial_println!(
            ":: WXN-M3B: xseg=[0x{:X},0x{:X}) wseg=none -> REFUSED (image declares no writable \
             PT_LOAD; no entry written) ::",
            x_lo, x_hi
        );
        return;
    };
    // Leg 3 — THE load-bearing assert, before a single entry is written. This function is a `.text`
    // address inside the extent it is about to restrict; if the phdr walk latched onto the wrong
    // segment it fires here rather than as a dead machine some instructions later.
    let here = wxn_ro_stage as *const () as u64;
    assert!(
        here >= x_lo && here < x_hi,
        "WXN-M3B: the derived executable extent [{:#x},{:#x}) does not contain this function \
         ({:#x}) — the read-only-LOAD phdr walk found the wrong segment",
        x_lo,
        x_hi,
        here
    );

    // The arm. `cfg!`, not `#[cfg]` — see the section header: the store below is compiled in every
    // `./arroyo check` leg whether or not this boot will execute it.
    let armed = cfg!(feature = "wxnro");
    let xpages = ((x_hi - x_lo) / PAGE_4K) as u32;
    let tramp = super::smp::TRAMPOLINE_ADDR as u64;

    // ---------------------------------------------------------------------------------------------
    // PRE-PASS — the census, read-only, before the write window exists. `leaf_entry_ptr` only reads
    // here; page tables are readable at ring 0 without touching `CR0.WP`.
    // ---------------------------------------------------------------------------------------------
    let (mut would, mut already_ro, mut wskip, mut absent) = (0u32, 0u32, 0u32, 0u32);
    let (mut huge_leaves, mut huge_pages) = (0u32, 0u32);
    // `pre_w` / `would_w` are folded-writability counts taken through `live_leaf`, so `expect_w`
    // below is MEASURED rather than assumed. A leaf whose W bit is set but whose parent folds W=0
    // contributes to `would` and not to `pre_w`, and clearing it moves nothing — without these two
    // counters `expect_w` would be wrong for exactly that page.
    let (mut pre_w, mut would_w) = (0u32, 0u32);
    let mut va = x_lo;
    while va < x_hi {
        let folds_w = live_leaf(va).map(|(_, acc)| acc.1).unwrap_or(false);
        if folds_w {
            pre_w += 1;
        }
        // (a) a page the image itself declares writable — leave it exactly as it is. This is the
        // round-IN the section header describes, and `wskip` is its witness.
        if va >= w_lo && va < w_hi {
            wskip += 1;
            va += PAGE_4K;
            continue;
        }
        // ONE WALKER: `leaf_entry_ptr` is the descent `set_framebuffer_wc` already uses, reused
        // rather than re-derived, so this file keeps one way of finding a leaf.
        match leaf_entry_ptr(va) {
            // (b) a huge leaf still covering part of the extent. M2 leaves a 2 MiB leaf whole when
            // the keep set covers all 512 of its pages, and such a leaf can extend past the W-clear
            // extent. Counted, and then refused below — clearing W across pages this stage never
            // examined is precisely the half-application it must not perform.
            Some((_, lvl)) if lvl != 1 => {
                let size = if lvl == 3 { 1u64 << 30 } else { 1u64 << 21 };
                let end = (va & !(size - 1)) + size;
                huge_leaves += 1;
                huge_pages += ((end.min(x_hi) - va) / PAGE_4K) as u32;
                va = end;
                continue;
            }
            Some((p, _)) => {
                // SAFETY: `p` is a live 4 KiB PTE inside the identity-mapped tables; read only.
                let e = unsafe { core::ptr::read_volatile(p) };
                if e & PTE_WRITABLE == 0 {
                    already_ro += 1;
                } else {
                    would += 1;
                    if folds_w {
                        would_w += 1;
                    }
                }
            }
            // (c) not mapped. Cannot happen for an image page — it is running — but a count is
            // cheaper than an argument.
            None => absent += 1,
        }
        va += PAGE_4K;
    }

    // The census, on the wire BEFORE anything is written. Everything a separate dry-run flight was
    // going to prove is on this line, and it survives a death anywhere below it.
    serial_println!(
        ":: WXN-M3B-PRE: xseg=[0x{:X},0x{:X}) xpages={} wseg=[0x{:X},0x{:X}) armed={} would={} \
         already_ro={} wskip={} absent={} huge_leaves={} huge_pages={} pre_w={} would_w={} \
         tramp=0x{:X} -> CENSUS ::",
        x_lo,
        x_hi,
        xpages,
        w_lo,
        w_hi,
        armed as u8,
        would,
        already_ro,
        wskip,
        absent,
        huge_leaves,
        huge_pages,
        pre_w,
        would_w,
        tramp,
    );

    // FAIL-CLOSED. The map must be exactly what the census models — all present 4 KiB leaves, every
    // non-`wskip` page writable — or M3b writes nothing at all. This is also what guarantees
    // `already_ro == 0`, and therefore that the rollback below restores exactly the set it cleared.
    if huge_leaves != 0 || absent != 0 || already_ro != 0 {
        serial_println!(
            ":: WXN-M3B: xseg=[0x{:X},0x{:X}) xpages={} armed={} huge_leaves={} absent={} \
             already_ro={} -> REFUSED (the executable extent is not the all-4K-present-writable map \
             this stage models; no entry written, map unmodified) ::",
            x_lo, x_hi, xpages, armed as u8, huge_leaves, absent, already_ro
        );
        return;
    }

    // The GR15 belt, M3b's own window. Unlike M2, M3b has NO legitimate reason to change the fb leaf:
    // the panel is device MMIO and the extent is the kernel image, asserted above to contain this
    // function. Any delta at all stops the boot. `fb=`/`fb_delta=`/`fb_chk=` go on the wire exactly as
    // M2 does it — without them a silently-skipped interlock (no panel, or `WRITER` contended) is
    // indistinguishable from a pass, which is the instrument-that-cannot-fire class this track has
    // paid for in GR13, GR15, GR18 and GR19. `fb_chk=1` means the comparison actually ran.
    let fb = crate::video::WRITER.try_lock().map(|w| w.base() as u64).unwrap_or(0);
    let fb_before = if fb != 0 { wx_probe_leaf(fb).map(|(e, l, _)| (e, l)) } else { None };

    // ---------------------------------------------------------------------------------------------
    // The edit. DISARMED, none of this runs: no `CR0.WP` window is opened and no second CR3 reload is
    // paid, because a dry run must not disarm the very protection it is a dry run for.
    // ---------------------------------------------------------------------------------------------
    let mut cleared = 0u32;
    if armed {
        with_page_tables_writable(|| {
            let mut va = x_lo;
            while va < x_hi {
                if va >= w_lo && va < w_hi {
                    va += PAGE_4K;
                    continue;
                }
                if let Some((p, 1)) = leaf_entry_ptr(va) {
                    // SAFETY: `p` is a live 4 KiB PTE inside the identity-mapped tables; CR0.WP is
                    // clear for this window so the firmware's read-only table pages accept the store;
                    // interrupts are masked by the wrapper. The write is `e & !PTE_WRITABLE` — a
                    // permission-only RMW that moves bit 1 and nothing else, so no address field, no
                    // PAT selector and no NX bit can be disturbed by it (the GR15 hazard needs a
                    // whole-entry write, which this is not).
                    unsafe {
                        let e = core::ptr::read_volatile(p);
                        if e & PTE_WRITABLE != 0 {
                            core::ptr::write_volatile(p, e & !PTE_WRITABLE);
                            invlpg(va);
                            cleared += 1;
                        }
                    }
                }
                va += PAGE_4K;
            }
        });
        wxn_flush_tlb(pge, cr4);
    }

    // The verify pass — a SECOND, INDEPENDENT count over the same set, through the FOLD. Runs in both
    // arms: disarmed it confirms the census's own model of the map, armed it confirms the flip.
    let mut verify_w = 0u32;
    {
        let mut va = x_lo;
        while va < x_hi {
            if let Some((_, acc)) = live_leaf(va)
                && acc.1
            {
                verify_w += 1;
            }
            va += PAGE_4K;
        }
    }
    // Measured, not assumed: armed, every page whose fold was writable stays writable except the
    // `would_w` subset this stage cleared; disarmed, nothing moved at all.
    let expect_w = if armed { pre_w - would_w } else { pre_w };

    // The GR15 belt, cashed. `delta == 0` is the only acceptable reading here (see above).
    let mut fb_chk = 0u8;
    let fb_delta = match (fb_before, if fb != 0 { wx_probe_leaf(fb) } else { None }) {
        (Some((e_before, l_before)), Some((e_after, l_after, _))) => {
            fb_chk = 1;
            assert!(
                l_after == l_before && e_after == e_before,
                "WXN-M3B: the framebuffer leaf changed across the W-clear — before=0x{:016X}/lvl{} \
                 after=0x{:016X}/lvl{}. M3b writes only inside the kernel image's own executable \
                 extent and clears only bit 1, so the panel's leaf cannot move; if it did, the extent \
                 or the walker is wrong about the map and this is the GR15 defect's shape.",
                e_before, l_before, e_after, l_after
            );
            e_after ^ e_before
        }
        (Some((e_before, _)), None) => panic!(
            "WXN-M3B: the framebuffer mapping at {:#x} is gone after the W-clear — it was present \
             (leaf 0x{:016X}) before it",
            fb, e_before
        ),
        _ => 0,
    };

    // ROLLBACK. The verify is the one check M3b can only make AFTER writing, so it is the one place
    // "refuse" has to mean "put it back". The restored set is exactly the cleared set: `wskip` pages
    // were never touched and `already_ro` is guaranteed 0 by the fail-closed refusal above, so every
    // non-`wskip` 4 KiB leaf in the extent was writable before this stage ran and must be writable
    // after it rolls back. Setting W is strictly permissive — it cannot fault — and it returns the
    // map to the state this kernel has booted in every day of its life.
    // `cleared != would` is the second trigger and it is a different failure than the verify: the
    // census and the write walk are two passes over the same set, taken with interrupts masked, on
    // one core, with nothing allocated in between — so they CANNOT legitimately disagree. If they do,
    // the map moved underneath this stage and nothing further it believes is worth acting on.
    if armed && (verify_w != expect_w || cleared != would) {
        let mut restored = 0u32;
        with_page_tables_writable(|| {
            let mut va = x_lo;
            while va < x_hi {
                if va >= w_lo && va < w_hi {
                    va += PAGE_4K;
                    continue;
                }
                if let Some((p, 1)) = leaf_entry_ptr(va) {
                    // SAFETY: as the clear above; `e | PTE_WRITABLE` moves bit 1 and nothing else.
                    unsafe {
                        let e = core::ptr::read_volatile(p);
                        if e & PTE_WRITABLE == 0 {
                            core::ptr::write_volatile(p, e | PTE_WRITABLE);
                            invlpg(va);
                            restored += 1;
                        }
                    }
                }
                va += PAGE_4K;
            }
        });
        wxn_flush_tlb(pge, cr4);
        serial_println!(
            ":: WXN-M3B: xseg=[0x{:X},0x{:X}) xpages={} armed=1 cleared={} would={} restored={} \
             verify_w={} expect_w={} pre_w={} would_w={} wskip={} fb=0x{:X} fb_delta=0x{:X} \
             fb_chk={} tramp=0x{:X} pge={} flush={} -> REFUSED (the post-write check did not close; \
             the W bits this stage cleared have been restored and the map is back where it started) ::",
            x_lo,
            x_hi,
            xpages,
            cleared,
            would,
            restored,
            verify_w,
            expect_w,
            pre_w,
            would_w,
            wskip,
            fb,
            fb_delta,
            fb_chk,
            tramp,
            pge as u8,
            if pge { "pge-toggle" } else { "cr3-reload" },
        );
        return;
    }

    // The verdict. `DRYRUN` is not a softer `RO`: it is the honest name for a boot that measured the
    // flip and did not perform it, and it must never be mistaken for one that did.
    let tramp_w = live_leaf(tramp).map(|(_, acc)| acc.1 as u8).unwrap_or(0);
    let verdict = if !armed {
        if verify_w != expect_w { "-> UNVERIFIED" } else { "-> DRYRUN" }
    } else if cleared == 0 {
        "-> VACUOUS"
    } else {
        "-> RO"
    };
    serial_println!(
        ":: WXN-M3B: xseg=[0x{:X},0x{:X}) xpages={} wseg=[0x{:X},0x{:X}) armed={} cleared={} would={} \
         already_ro={} wskip={} absent={} huge_leaves={} huge_pages={} verify_w={} expect_w={} \
         pre_w={} would_w={} fb=0x{:X} fb_delta=0x{:X} fb_chk={} tramp=0x{:X} tramp_w={} pge={} \
         flush={} {} ::",
        x_lo,
        x_hi,
        xpages,
        w_lo,
        w_hi,
        armed as u8,
        cleared,
        would,
        already_ro,
        wskip,
        absent,
        huge_leaves,
        huge_pages,
        verify_w,
        expect_w,
        pre_w,
        would_w,
        fb,
        fb_delta,
        fb_chk,
        tramp,
        tramp_w,
        pge as u8,
        if pge { "pge-toggle" } else { "cr3-reload" },
        verdict,
    );
    // M3b's self-prediction, the twin of M1's `residue_leaves` and M2's `keep_x`. Every identity is
    // INTRA-boot (see the section header on why a cross-boot comparison against a disarmed flight is
    // unsound — the two arms are different binaries and this commit's own QEMU pair differs by a page):
    //   * the census closure, internal to the WXN-M3B-PRE line:
    //       would + already_ro + wskip + absent + huge_pages == xpages
    //   * prediction vs result, across this stage's own two lines, same binary:
    //       cleared == would          (armed; enforced above — a mismatch rolls back and refuses)
    //   * the audit identity, which REPLACES `kern_WX == keep_x` from M3b onward:
    //       kern_WX == keep_x - already_ro - cleared
    //     Disarmed, `cleared == 0` and it degenerates to the pre-M3b identity, which is what makes a
    //     disarmed boot a true no-op on the analyzer as well as on the map.
    // `keep_x` is counted by M2 during its edit, `would` by this stage's census walk, `cleared` by its
    // write walk, and `kern_WX` by the WXAUDIT walk one screen down — four walkers, one number.
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
