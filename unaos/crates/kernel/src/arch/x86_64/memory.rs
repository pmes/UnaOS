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
