// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — shared-core
//!
//! SELFBUILD6 (B360): the x86-64 static TLS layout (variant II: every block sits BELOW the thread pointer) for a set of
//! modules, laid out so that musl's STATIC `__init_tls` builds exactly it from ONE synthetic `PT_TLS`.
//!
//! The formula musl needs. musl's static start-up reads one `PT_TLS {vaddr I, filesz, memsz S, align A}` and puts the
//! block at `tp - S'` with `S' = S + ((-S - I) & (A-1))`, copying `filesz` bytes of the image there; `tp` itself is
//! aligned to `A`. The loader therefore builds a TEMPLATE of `S` bytes at an `A`-aligned address `I` with `S` a multiple
//! of `A` (so `S' = S`), holding every module's `.tdata` at `S - d(m)`, where `d(m)` is the module's distance below `tp`:
//!
//! * the EXECUTABLE goes first (`d = memsz + ((-memsz - p_vaddr) & (align-1))`), because a linker resolves its
//!   local-exec accesses to exactly that offset (lld `getTlsTpOffset`, x86-64);
//! * every shared object after it, `d = c + ((-c - p_vaddr) & (align-1))` with `c = d(prev) + memsz`, so that
//!   `tp - d ≡ p_vaddr (mod align)`;
//! * then [`SURPLUS`] bytes, the room a later `dlopen` takes its blocks from.
//!
//! A module's TP offset (what `R_X86_64_TPOFF64` adds and the loader's `__tls_get_addr` reads) is `-d`.

/// Room for the TLS of objects `dlopen`ed later (proc-macros): 16 KiB, as the design says.
pub const SURPLUS: u64 = 16384;
/// The template's (and so the thread pointer's) alignment: every module's `p_align` must divide it.
pub const ALIGN: u64 = 64;

/// One module's `PT_TLS`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mod {
    pub vaddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub align: u64,
}

/// The distance below `tp` of the next module, placed after a module ending `cur` bytes below it.
pub fn place(cur: u64, m: &Mod) -> Option<u64> {
    let a = m.align.max(1);
    if !a.is_power_of_two() || a > ALIGN {
        return None;
    }
    let c = cur.checked_add(m.memsz)?;
    Some(c + (0u64.wrapping_sub(c).wrapping_sub(m.vaddr) & (a - 1)))
}

/// The layout of the initial modules: each one's distance below `tp`, the bytes they use, and the template size.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    /// `d(m)` per module, in the order given (`None` = the module has no TLS).
    pub dist: alloc::vec::Vec<Option<u64>>,
    /// The distance the last placed module ends at (a later `dlopen` places after it).
    pub used: u64,
    /// The template's size `S` (a multiple of [`ALIGN`]).
    pub size: u64,
}

/// Lay out `mods` (the executable first, then load order).
pub fn layout(mods: &[Option<Mod>]) -> Option<Layout> {
    let mut cur = 0u64;
    let mut dist = alloc::vec::Vec::with_capacity(mods.len());
    for m in mods {
        match m {
            Some(m) => {
                cur = place(cur, m)?;
                dist.push(Some(cur));
            }
            None => dist.push(None),
        }
    }
    let size = (cur.checked_add(SURPLUS)? + ALIGN - 1) & !(ALIGN - 1);
    Some(Layout { dist, used: cur, size })
}

/// What musl's static `__init_tls` computes for a `PT_TLS` of `size` at `image` aligned `align` — the block's distance below
/// `tp`. The loader's template must make this equal `size`.
pub fn musl_offset(image: u64, size: u64, align: u64) -> u64 {
    size + (0u64.wrapping_sub(size).wrapping_sub(image) & (align.max(1) - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_matches_lld() {
        // rust-lld's PT_TLS: vaddr 0x8a99590, memsz 0x4c0, align 8 -> lld puts it at tp - 0x4c0 (memsz is a multiple of 8
        // and (-memsz - vaddr) & 7 = 0).
        let m = Mod { vaddr: 0x8a99590, filesz: 4, memsz: 0x4c0, align: 8 };
        assert_eq!(place(0, &m), Some(0x4c0));
        // A misaligned vaddr pads: tp - d must be ≡ vaddr (mod align).
        let m = Mod { vaddr: 0x1004, filesz: 0, memsz: 0x10, align: 16 };
        let d = place(0, &m).unwrap();
        assert_eq!(0u64.wrapping_sub(d) & 15, 4);
    }

    #[test]
    fn driver_layout_and_musl_agree() {
        // rustc (no TLS), librustc_driver (vaddr 0xd8b9ac8, memsz 0x3428, align 8), libgcc_s (none), libc.so (none).
        let drv = Mod { vaddr: 0xd8b9ac8, filesz: 0xf0, memsz: 0x3428, align: 8 };
        let l = layout(&[None, Some(drv), None, None]).unwrap();
        assert_eq!(l.dist, alloc::vec![None, Some(0x3428), None, None]);
        assert_eq!(l.size % ALIGN, 0);
        assert!(l.size >= l.used + SURPLUS);
        // The template sits page-aligned: musl's offset is then exactly the template size.
        assert_eq!(musl_offset(0x7f00_0000_0000, l.size, ALIGN), l.size);
        // Each block's start is aligned as its module asks, for any ALIGN-aligned tp.
        let tp = 0x7f00_1234_0000u64;
        assert_eq!((tp - 0x3428) % 8, 0xd8b9ac8 % 8);
    }

    #[test]
    fn refuses_overaligned() {
        assert_eq!(place(0, &Mod { vaddr: 0, filesz: 0, memsz: 8, align: 128 }), None);
    }
}
