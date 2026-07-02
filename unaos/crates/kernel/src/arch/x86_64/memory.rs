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
