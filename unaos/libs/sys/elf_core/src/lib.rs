// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — shared-core
//!
//! STORMFAULT (rmbp-ledger B351): the ring-3 ELF SEGMENT PLAN, one implementation for both kernel loaders
//! (x86 `arch/x86_64/elf.rs`, aarch64 `arch/aarch64/xwin.rs`) and the host tests.
//!
//! Given a program's `PT_LOAD` segments (as WINDOW OFFSETS) and its `PT_GNU_STACK` request, [`plan`] decides
//! where every byte the program owns lives — the file-backed span, the zero-filled `.bss` tail
//! (`p_memsz > p_filesz`, the ELF rule), the stack and its guard page — and REFUSES with a named
//! [`PlanErr`] any load whose segments would leave the window, overlap each other, overlap the args page, or
//! collide with the stack, instead of letting the first write fault in ring 3. The plan's [`FaultMap`] is
//! what the ring-3 fault line reads to name the segment a faulting address fell in (`seg=bss|stack|none`).
//!
//! The loaders keep their own byte-level ELF parsing (magic, class, machine, W^X); this core is the
//! arithmetic both of them must agree on. No dependencies, no allocator, no `unsafe`.

#![no_std]
#![forbid(unsafe_code)]

/// `p_type` — a loadable segment.
pub const PT_LOAD: u32 = 1;
/// `p_type` — the stack request (`-z stack-size=` writes its `p_memsz`).
pub const PT_GNU_STACK: u32 = 0x6474_E551;
/// `p_flags` — executable.
pub const PF_X: u32 = 0x1;
/// `p_flags` — writable.
pub const PF_W: u32 = 0x2;
/// The most `PT_LOAD` segments a plan carries (the loaders' own ceiling).
pub const MAX_SEGS: usize = 8;
/// The page.
pub const PAGE: u64 = 4096;

/// One `PT_LOAD` segment, its address already a WINDOW OFFSET (VA − the slot's window base).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Seg {
    pub vaddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub flags: u32,
}

/// A half-open byte range `[lo, hi)` of window offsets. Empty when `lo >= hi`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Range {
    pub lo: u64,
    pub hi: u64,
}

impl Range {
    pub const EMPTY: Range = Range { lo: 0, hi: 0 };
    pub const fn is_empty(&self) -> bool {
        self.lo >= self.hi
    }
    pub const fn contains(&self, x: u64) -> bool {
        x >= self.lo && x < self.hi
    }
    pub const fn overlaps(&self, o: &Range) -> bool {
        !self.is_empty() && !o.is_empty() && self.lo < o.hi && o.lo < self.hi
    }
}

/// The window a plan is made against, all in window offsets.
#[derive(Clone, Copy, Debug)]
pub struct Window {
    /// The window's first byte (0 for the classic fixed window; `USER_XWIN_OFF` for the ELF window).
    pub lo: u64,
    /// One past the window's last byte; the initial stack pointer sits here.
    pub hi: u64,
    /// The args page (`USER_ARGS_OFF`, one page) — no segment, stack or guard may cover it.
    pub args: Range,
    /// true = the ELF window: a page-granular stack of `stack_default` (or the request), with an unmapped
    /// guard page beneath it. false = the classic fixed window: the stack is whatever lies above the image
    /// (or exactly the request), no guard.
    pub elf: bool,
    /// Refuse two PT_LOADs whose memsz spans share a byte. true for the ELF window (whose pages take one
    /// right each); false for the classic window, whose per-segment re-protect is the WINX-2 contract and
    /// whose WINX-3 edge rung deliberately stretches text over data.
    pub refuse_overlap: bool,
    pub stack_default: u64,
    pub stack_max: u64,
}

/// Why a plan was refused. Every variant names the rule; [`PlanErr::as_str`] is the operator string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanErr {
    NoSegments,
    TooManySegments,
    FileszOverMemsz,
    SpanOverflow,
    BelowWindow,
    LeavesWindow,
    SegmentsOverlap,
    OverlapsArgs,
    StackTooBig,
    StackOverlapsImage,
}

impl PlanErr {
    pub const fn as_str(self) -> &'static str {
        match self {
            PlanErr::NoSegments => "STORMFAULT: no PT_LOAD segments",
            PlanErr::TooManySegments => "STORMFAULT: too many PT_LOAD segments",
            PlanErr::FileszOverMemsz => "STORMFAULT: p_filesz > p_memsz",
            PlanErr::SpanOverflow => "STORMFAULT: segment span overflows",
            PlanErr::BelowWindow => "STORMFAULT: segment starts below the window",
            PlanErr::LeavesWindow => "STORMFAULT: segment memsz (bss) leaves the window",
            PlanErr::SegmentsOverlap => "STORMFAULT: two PT_LOAD segments overlap",
            PlanErr::OverlapsArgs => "STORMFAULT: segment or stack overlaps the args page",
            PlanErr::StackTooBig => "STORMFAULT: PT_GNU_STACK request exceeds the stack cap",
            PlanErr::StackOverlapsImage => "STORMFAULT: stack (PT_GNU_STACK) collides with the image's bss",
        }
    }
    /// The short wire word (`reason=` on the loader's refusal line).
    pub const fn word(self) -> &'static str {
        match self {
            PlanErr::NoSegments => "no-segments",
            PlanErr::TooManySegments => "too-many-segments",
            PlanErr::FileszOverMemsz => "filesz-over-memsz",
            PlanErr::SpanOverflow => "span-overflow",
            PlanErr::BelowWindow => "below-window",
            PlanErr::LeavesWindow => "leaves-window",
            PlanErr::SegmentsOverlap => "segments-overlap",
            PlanErr::OverlapsArgs => "overlaps-args",
            PlanErr::StackTooBig => "stack-too-big",
            PlanErr::StackOverlapsImage => "stack-overlaps-image",
        }
    }
}

/// The ranges the fault line classifies against: up to two `.bss` tails and the stack (guard included —
/// a write into the guard is a stack overflow). Window offsets; empty ranges match nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FaultMap {
    pub bss: [Range; 2],
    pub stack: Range,
}

impl FaultMap {
    pub const NONE: FaultMap = FaultMap { bss: [Range::EMPTY; 2], stack: Range::EMPTY };
}

/// What a faulting address fell in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegKind {
    Bss,
    Stack,
    None,
}

impl SegKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            SegKind::Bss => "bss",
            SegKind::Stack => "stack",
            SegKind::None => "none",
        }
    }
}

/// Name the segment window offset `off` lies in.
pub fn classify(map: &FaultMap, off: u64) -> SegKind {
    if map.bss.iter().any(|r| r.contains(off)) {
        SegKind::Bss
    } else if map.stack.contains(off) {
        SegKind::Stack
    } else {
        SegKind::None
    }
}

/// A validated plan: every segment's full memsz span, its zero-filled tail, the stack, the guard.
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub min_vaddr: u64,
    /// The highest segment end (window offset).
    pub max_end: u64,
    /// The zero-filled tails `[vaddr + filesz, vaddr + memsz)`, one per segment that has one.
    pub bss: [Range; MAX_SEGS],
    pub nbss: usize,
    /// The stack: `[sp_lo, window hi)`.
    pub stack: Range,
    /// The unmapped guard page under an ELF-window stack (empty in the fixed window).
    pub guard: Range,
    /// The stack bytes (page-rounded in the ELF window).
    pub stack_bytes: u64,
}

impl Plan {
    /// The ranges the fault line reads (the first two bss tails; the stack with its guard).
    pub fn fault_map(&self) -> FaultMap {
        let mut m = FaultMap::NONE;
        for (i, r) in self.bss[..self.nbss].iter().take(2).enumerate() {
            m.bss[i] = *r;
        }
        m.stack = if self.guard.is_empty() { self.stack } else { Range { lo: self.guard.lo, hi: self.stack.hi } };
        m
    }
    /// The total zero-filled bytes (for the witness line).
    pub fn bss_bytes(&self) -> u64 {
        self.bss[..self.nbss].iter().map(|r| r.hi - r.lo).sum()
    }
}

const fn page_up(x: u64) -> u64 {
    (x + PAGE - 1) & !(PAGE - 1)
}

/// Make the plan. `segs` are window offsets; `stack_req` is `PT_GNU_STACK`'s `p_memsz` (0 = undeclared).
pub fn plan(segs: &[Seg], stack_req: u64, w: &Window) -> Result<Plan, PlanErr> {
    if segs.is_empty() {
        return Err(PlanErr::NoSegments);
    }
    if segs.len() > MAX_SEGS {
        return Err(PlanErr::TooManySegments);
    }
    let mut p = Plan {
        min_vaddr: u64::MAX,
        max_end: 0,
        bss: [Range::EMPTY; MAX_SEGS],
        nbss: 0,
        stack: Range::EMPTY,
        guard: Range::EMPTY,
        stack_bytes: 0,
    };
    for (i, s) in segs.iter().enumerate() {
        if s.filesz > s.memsz {
            return Err(PlanErr::FileszOverMemsz);
        }
        let end = s.vaddr.checked_add(s.memsz).ok_or(PlanErr::SpanOverflow)?;
        if s.vaddr < w.lo {
            return Err(PlanErr::BelowWindow);
        }
        if end > w.hi {
            return Err(PlanErr::LeavesWindow);
        }
        let span = Range { lo: s.vaddr, hi: end };
        if span.overlaps(&w.args) {
            return Err(PlanErr::OverlapsArgs);
        }
        for o in segs[..i].iter().filter(|_| w.refuse_overlap) {
            if span.overlaps(&Range { lo: o.vaddr, hi: o.vaddr + o.memsz }) {
                return Err(PlanErr::SegmentsOverlap);
            }
        }
        if s.memsz > s.filesz {
            p.bss[p.nbss] = Range { lo: s.vaddr + s.filesz, hi: end };
            p.nbss += 1;
        }
        p.min_vaddr = p.min_vaddr.min(s.vaddr);
        p.max_end = p.max_end.max(end);
    }
    if w.elf {
        let bytes = if stack_req == 0 { w.stack_default } else { page_up(stack_req) };
        if bytes > w.stack_max || bytes + PAGE > w.hi - w.lo {
            return Err(PlanErr::StackTooBig);
        }
        p.stack = Range { lo: w.hi - bytes, hi: w.hi };
        p.guard = Range { lo: p.stack.lo - PAGE, hi: p.stack.lo };
        p.stack_bytes = bytes;
        if p.max_end > p.guard.lo {
            return Err(PlanErr::StackOverlapsImage);
        }
    } else if stack_req == 0 {
        // The classic window: the loader parks sp at the top and the stack is what the image leaves.
        p.stack = Range { lo: (p.max_end + 15) & !15, hi: w.hi };
        p.stack_bytes = w.hi.saturating_sub(p.stack.lo);
    } else {
        if stack_req > w.stack_max || stack_req > w.hi - w.lo {
            return Err(PlanErr::StackTooBig);
        }
        p.stack = Range { lo: (w.hi - stack_req) & !15, hi: w.hi };
        p.stack_bytes = w.hi - p.stack.lo;
        if p.stack.lo < p.max_end {
            return Err(PlanErr::StackOverlapsImage);
        }
    }
    if p.stack.overlaps(&w.args) || p.guard.overlaps(&w.args) {
        return Err(PlanErr::OverlapsArgs);
    }
    Ok(p)
}

/// Read the `PT_LOAD` segments and the `PT_GNU_STACK` request out of a little-endian ELF64 image's program
/// headers (bounds-checked; no machine/type checks — the loaders make those). Addresses are returned as
/// they are in the file; the caller rebases them to window offsets. `None` for a malformed table.
pub fn read_phdrs(b: &[u8], out: &mut [Seg; MAX_SEGS]) -> Option<(usize, u64)> {
    let rd = |o: usize, n: usize| -> Option<u64> {
        let s = b.get(o..o.checked_add(n)?)?;
        let mut v = 0u64;
        for (i, &c) in s.iter().enumerate() {
            v |= (c as u64) << (8 * i);
        }
        Some(v)
    };
    if b.len() < 64 || b[0..4] != [0x7F, b'E', b'L', b'F'] || b[4] != 2 || b[5] != 1 {
        return None;
    }
    let (phoff, phent, phnum) = (rd(32, 8)? as usize, rd(54, 2)? as usize, rd(56, 2)? as usize);
    if phent != 56 {
        return None;
    }
    let (mut n, mut stack) = (0usize, 0u64);
    for i in 0..phnum {
        let ph = phoff.checked_add(i.checked_mul(56)?)?;
        let t = rd(ph, 4)? as u32;
        if t == PT_GNU_STACK {
            stack = rd(ph + 40, 8)?;
        } else if t == PT_LOAD {
            if n >= MAX_SEGS {
                return None;
            }
            out[n] = Seg { flags: rd(ph + 4, 4)? as u32, vaddr: rd(ph + 16, 8)?, filesz: rd(ph + 32, 8)?, memsz: rd(ph + 40, 8)? };
            n += 1;
        }
    }
    Some((n, stack))
}

#[cfg(test)]
mod tests {
    use super::*;

    // The ABI numbers (una_abi; restated here because this core takes no dependencies — the kernel asserts
    // they agree at compile time in `arch/x86_64/elf.rs`).
    const FIXED_BYTES: u64 = 16 << 10;
    const XWIN_OFF: u64 = 0x20_0000;
    const XWIN_BYTES: u64 = 4 << 20;
    const ARGS: Range = Range { lo: 0x1F_F000, hi: 0x20_0000 };
    const FIXED: Window = Window { lo: 0, hi: FIXED_BYTES, args: ARGS, elf: false, refuse_overlap: false, stack_default: 0, stack_max: 1 << 20 };
    const ELFW: Window = Window { lo: XWIN_OFF, hi: XWIN_OFF + XWIN_BYTES, args: ARGS, elf: true, refuse_overlap: true, stack_default: 64 << 10, stack_max: 1 << 20 };

    /// VUG-X86.ELF (staged as APPS/VUG.ELF) on this tree, `readelf -lW`: entry 0xe8 (THREE program headers
    /// since EXECNAME's PT_NOTE — 64 + 3*56), no PT_GNU_STACK.
    ///   LOAD 0x000000 0x0 0x0 0x002af8 0x002af8 R E
    ///   LOAD 0x003000 0x3000 0x3000 0x00001c 0x000514 RW
    const VUG: [Seg; 2] = [
        Seg { vaddr: 0, filesz: 0x2af8, memsz: 0x2af8, flags: 5 },
        Seg { vaddr: 0x3000, filesz: 0x1c, memsz: 0x514, flags: 6 },
    ];

    #[test]
    fn vug_plan_maps_its_bss_and_names_the_boot21_corpse_none() {
        let p = plan(&VUG, 0, &FIXED).expect("VUG.ELF plans in the fixed window");
        assert_eq!(p.max_end, 0x3514);
        assert_eq!(p.nbss, 1);
        assert_eq!(p.bss[0], Range { lo: 0x301c, hi: 0x3514 });
        assert_eq!(p.stack, Range { lo: 0x3520, hi: 0x4000 });
        let m = p.fault_map();
        // The boot-21 fault: cr2 = base + 0x56000 — the page after surface slot 0 (base + 0x5000 + 288*288*4),
        // NOT a bss or stack page: the loader is not what faulted (see STORMFAULT.md).
        assert_eq!(classify(&m, 0x56000), SegKind::None);
        assert_eq!(classify(&m, 0x3513), SegKind::Bss);
        assert_eq!(classify(&m, 0x3fff), SegKind::Stack);
        assert_eq!(classify(&m, 0x2af0), SegKind::None);
        std::println!(":: STORMFAULT-HOST: vug max_end=0x3514 bss=[0x301c,0x3514) stack=[0x3520,0x4000) cr2_off=0x56000 seg=none -> PASS ::");
    }

    /// LUMEN-X86.ELF (APPS/LUMEN.ELF) on this tree, `readelf -lW`, rebased by USER_BASE (0x10000000000):
    ///   LOAD 0x001000 0x10000200000 filesz 0x02eb05 memsz 0x02eb05 R E
    ///   LOAD 0x02fb08 0x1000022eb08 filesz 0x0032de memsz 0x0032de R
    ///   LOAD 0x033000 0x10000232000 filesz 0x000790 memsz 0x2ee0a0 RW
    ///   GNU_STACK memsz 0x40000
    const LUMEN: [Seg; 3] = [
        Seg { vaddr: 0x20_0000, filesz: 0x2eb05, memsz: 0x2eb05, flags: 5 },
        Seg { vaddr: 0x22_eb08, filesz: 0x32de, memsz: 0x32de, flags: 4 },
        Seg { vaddr: 0x23_2000, filesz: 0x790, memsz: 0x2e_e0a0, flags: 6 },
    ];

    #[test]
    fn lumen_plan_honours_gnu_stack_and_a_3mib_bss() {
        let p = plan(&LUMEN, 0x40000, &ELFW).expect("LUMEN.ELF plans in the ELF window");
        assert_eq!(p.bss[0], Range { lo: 0x23_2790, hi: 0x52_00a0 });
        assert_eq!(p.stack_bytes, 0x40000);
        assert_eq!(p.stack, Range { lo: 0x5C_0000, hi: 0x60_0000 });
        assert_eq!(p.guard, Range { lo: 0x5B_F000, hi: 0x5C_0000 });
        assert_eq!(classify(&p.fault_map(), 0x5B_F800), SegKind::Stack); // the guard = an overflow
        assert_eq!(classify(&p.fault_map(), 0x52_0000), SegKind::Bss);
        assert_eq!(classify(&p.fault_map(), 0x53_0000), SegKind::None); // the heap
    }

    #[test]
    fn elfbss_fixture_shape() {
        // `tests elfbss`: text page + a 64 KiB filesz-0 bss + PT_GNU_STACK 64 KiB.
        let segs = [Seg { vaddr: XWIN_OFF, filesz: 0x100, memsz: 0x100, flags: 5 }, Seg { vaddr: XWIN_OFF + 0x1000, filesz: 0, memsz: 0x10000, flags: 6 }];
        let p = plan(&segs, 0x10000, &ELFW).unwrap();
        assert_eq!(p.bss_bytes(), 0x10000);
        assert_eq!(classify(&p.fault_map(), XWIN_OFF + 0x10FFF), SegKind::Bss);
    }

    #[test]
    fn refusals_are_named() {
        // bss past the window end.
        let big = [Seg { vaddr: XWIN_OFF, filesz: 0x10, memsz: XWIN_BYTES + 1, flags: 6 }];
        assert_eq!(plan(&big, 0, &ELFW).unwrap_err(), PlanErr::LeavesWindow);
        // bss running into the stack guard.
        let near = [Seg { vaddr: XWIN_OFF, filesz: 0x10, memsz: XWIN_BYTES - 0x10000, flags: 6 }];
        assert_eq!(plan(&near, 0, &ELFW).unwrap_err(), PlanErr::StackOverlapsImage);
        // over the args page (a window that holds it).
        let wide = Window { lo: 0, hi: XWIN_OFF + XWIN_BYTES, ..ELFW };
        let a = [Seg { vaddr: 0x1F_E000, filesz: 0, memsz: 0x2000, flags: 6 }];
        assert_eq!(plan(&a, 0, &wide).unwrap_err(), PlanErr::OverlapsArgs);
        // overlap: refused in the ELF window; the classic window keeps WINX-3's edge rung (text over data) loading.
        let o = [Seg { vaddr: XWIN_OFF, filesz: 0, memsz: 0x2000, flags: 5 }, Seg { vaddr: XWIN_OFF + 0x1000, filesz: 0, memsz: 0x10, flags: 6 }];
        assert_eq!(plan(&o, 0, &ELFW).unwrap_err(), PlanErr::SegmentsOverlap);
        let edge = [Seg { vaddr: 0, filesz: 0x40, memsz: 0x3000, flags: 5 }, Seg { vaddr: 0x1000, filesz: 0, memsz: 0x1000, flags: 6 }];
        assert!(plan(&edge, 0, &FIXED).is_ok());
        // the fixed window: a declared stack that would cover VUG's bss.
        assert_eq!(plan(&VUG, 0x1000, &FIXED).unwrap_err(), PlanErr::StackOverlapsImage);
        assert!(plan(&VUG, 0xA00, &FIXED).is_ok());
        assert_eq!(plan(&LUMEN, 2 << 20, &ELFW).unwrap_err(), PlanErr::StackTooBig);
        let f = [Seg { vaddr: 0, filesz: 2, memsz: 1, flags: 5 }];
        assert_eq!(plan(&f, 0, &FIXED).unwrap_err(), PlanErr::FileszOverMemsz);
        assert_eq!(plan(&[], 0, &FIXED).unwrap_err(), PlanErr::NoSegments);
    }

    #[test]
    fn read_phdrs_round_trip() {
        // A 3-header image (text, data, GNU_STACK) built in place.
        let mut b = [0u8; 64 + 3 * 56];
        b[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
        b[4] = 2;
        b[5] = 1;
        b[32..40].copy_from_slice(&64u64.to_le_bytes());
        b[54..56].copy_from_slice(&56u16.to_le_bytes());
        b[56..58].copy_from_slice(&3u16.to_le_bytes());
        let put = |b: &mut [u8], i: usize, t: u32, f: u32, va: u64, fs: u64, ms: u64| {
            let p = 64 + i * 56;
            b[p..p + 4].copy_from_slice(&t.to_le_bytes());
            b[p + 4..p + 8].copy_from_slice(&f.to_le_bytes());
            b[p + 16..p + 24].copy_from_slice(&va.to_le_bytes());
            b[p + 32..p + 40].copy_from_slice(&fs.to_le_bytes());
            b[p + 40..p + 48].copy_from_slice(&ms.to_le_bytes());
        };
        put(&mut b, 0, PT_LOAD, 5, 0, 0x2af8, 0x2af8);
        put(&mut b, 1, PT_LOAD, 6, 0x3000, 0x1c, 0x514);
        put(&mut b, 2, PT_GNU_STACK, 6, 0, 0, 0x800);
        let mut s = [Seg::default(); MAX_SEGS];
        let (n, st) = read_phdrs(&b, &mut s).unwrap();
        assert_eq!((n, st), (2, 0x800));
        assert_eq!(&s[..2], &VUG);
    }

    /// When `STORMFAULT_VUG` / `STORMFAULT_LUMEN` name the built images (the arroyo lines), plan the REAL bytes.
    #[test]
    fn real_images_when_present() {
        for (var, elf) in [("STORMFAULT_VUG", false), ("STORMFAULT_LUMEN", true)] {
            let Ok(path) = std::env::var(var) else { continue };
            let b = std::fs::read(&path).expect("image readable");
            let mut s = [Seg::default(); MAX_SEGS];
            let (n, st) = read_phdrs(&b, &mut s).expect("phdrs");
            let base = if elf { 0x100_0000_0000u64 } else { 0 };
            for x in &mut s[..n] {
                x.vaddr -= base;
            }
            let p = plan(&s[..n], st, if elf { &ELFW } else { &FIXED }).expect("real image plans");
            std::println!(":: STORMFAULT-HOST: {} segs={} max_end={:#x} bss={} stack={} -> PASS ::", path, n, p.max_end, p.bss_bytes(), p.stack_bytes);
        }
    }
}

#[cfg(test)]
extern crate std;
