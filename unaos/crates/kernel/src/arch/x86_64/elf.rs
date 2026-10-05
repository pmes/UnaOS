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

//! WINX-2 — the x86_64 ring-3 ELF64 loader.
//!
//! The x86 twin of the aarch64 EXEC-1 loader (`arch/aarch64/syscall.rs`: `validate_elf` /
//! `map_image_into_slot`). Until this module, x86 had NO ring-3 program loader at all: every ring-3
//! program on x86 was either a kernel-embedded inline asm fixture or the single flat `HELLO.BIN` blob the
//! U2 path copies into a code page. `run`/`bg` were `#[cfg(feature = "baremetal")]`, i.e. aarch64
//! Pi-4-only, and `crates/user-stat` had nowhere to land.
//!
//! DELIBERATELY MINIMAL, and exactly as minimal as the aarch64 twin: static `ET_EXEC` only, no dynamic
//! linking, no relocations, no interpreter, no PIE. It maps `PT_LOAD` and nothing else. Every rejection
//! the aarch64 validator makes, this one makes, with the same message — the two were written against the
//! same checklist so a program accepted on one arch is accepted on the other for the same reasons.
//!
//! # Why this is a twin and not shared code
//! The validator below is arch-neutral apart from one line (`e_machine`), and duplicating ~90 lines of
//! security-critical validation is a real cost. It is duplicated anyway because the aarch64 original lives
//! private inside `arch/aarch64/syscall.rs`, which is outside this arc's lane, and hoisting it would edit
//! a file another track owns. The intended end state is one `crate::elf` both arches call, with
//! `e_machine` passed in; this module is written to make that fold mechanical (the only arch-specific
//! items are `EM_X86_64` and the mapping half). Flagged for the integrator.
//!
//! # The untrusted-bytes contract
//! The image is UNTRUSTED. Nothing about it is believed beyond what is checked here: every field read is
//! bounds-checked against the image slice, every segment's file range must lie inside the image, every
//! segment's mapped span must fit the slot's user window, and W^X is enforced per segment BEFORE anything
//! is mapped. Validation completes BEFORE a slot is allocated (the "slot allocated LAST" invariant), so a
//! rejected image leaks no resource. The bytes then run only in ring 3, in a private address space, under
//! the fault-kill net.

use super::memory;

// -------------------------------------------------------------------------------------------------
// ELF64 constants (the subset a minimal static loader needs).
// -------------------------------------------------------------------------------------------------
const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ELFCLASS64: u8 = 2; // e_ident[EI_CLASS]
const ELFDATA2LSB: u8 = 1; // e_ident[EI_DATA] — little-endian
const ET_EXEC: u16 = 2; // e_type — a fixed-address executable (no PIE/DYN)
/// e_machine. The ONE line that differs from the aarch64 twin (which wants 183 / EM_AARCH64). An
/// aarch64 image handed to this loader is rejected here rather than being mapped and faulting in ring 3.
const EM_X86_64: u16 = 62; // 0x3E
const PT_LOAD: u32 = 1; // p_type — a loadable segment
pub const PF_X: u32 = 0x1; // p_flags — executable
pub const PF_W: u32 = 0x2; // p_flags — writable
const EHDR_SIZE: usize = 64; // sizeof(Elf64_Ehdr)
const PHDR_SIZE: usize = 56; // sizeof(Elf64_Phdr)
/// A minimal loader ceiling (a real image here has 2 — text and data); more is rejected, fail-closed.
const MAX_LOAD_SEGS: usize = 8;

/// One validated `PT_LOAD` segment: its file offset/size, virtual address, in-memory size (>= filesz for
/// a `.bss` tail) and flags.
#[derive(Clone, Copy)]
pub struct ElfSeg {
    pub off: usize,
    pub vaddr: u64,
    pub filesz: usize,
    pub memsz: usize,
    pub flags: u32,
}

/// A validated load plan: the entry VA, the lowest `PT_LOAD` vaddr (the load bias is
/// `window_base - min_vaddr`, so `p_vaddr` are treated as offsets from the window base — the linker
/// scripts link at 0), and the collected segments. Fixed-size array, no heap.
pub struct ElfPlan {
    pub entry: u64,
    pub min_vaddr: u64,
    pub segs: [ElfSeg; MAX_LOAD_SEGS],
    pub nsegs: usize,
    /// RING3WIN: true = the elf model (lowest PT_LOAD at or above `USER_BASE + memory::XWIN_OFF`; p_vaddr
    /// are ABSOLUTE ring-3 VAs inside the ELF window, linked there). false = the fixed 16 KiB model. In an
    /// elf-model plan `entry`, `min_vaddr`, `max_end` and every `segs[i].vaddr` are rebased to WINDOW
    /// OFFSETS (VA - USER_BASE) by `validate_elf_model`, so the mapper speaks offsets in both models.
    pub model_elf: bool,
    /// RING3WIN: the elf model's stack bytes (PT_GNU_STACK p_memsz page-rounded, or the 64 KiB default).
    pub stack: usize,
    /// RING3WIN: the elf model's highest segment end (window offset) — the heap starts at the next page.
    pub max_end: u64,
    /// STORMFAULT (B351): the `elf_core` plan's fault map (bss tails, stack + guard; window offsets) — what
    /// the ring-3 fault line names `seg=` from once the image is placed.
    pub fmap: elf_core::FaultMap,
}

/// RING3WIN: `PT_GNU_STACK` — its p_memsz (set by `-z stack-size=`) is the elf model's stack request.
const PT_GNU_STACK: u32 = 0x6474_E551;

/// True iff `b` begins with the ELF magic (`\x7fELF`). The dispatch between the ELF loader and the flat
/// fallback — a flat `.BIN` never begins with this magic (the x86 blobs start with `xor`/`mov`), so they
/// route to the flat path unchanged.
pub fn is_elf_image(b: &[u8]) -> bool {
    b.len() >= 4 && b[0..4] == ELF_MAGIC
}

// Little-endian field reads, bounds-checked against the image slice (a malformed ELF must never index
// out of bounds — this is the first line of defence on untrusted input).
fn rd_u16(b: &[u8], off: usize) -> Option<u16> {
    b.get(off..off + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}
fn rd_u32(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}
fn rd_u64(b: &[u8], off: usize) -> Option<u64> {
    b.get(off..off + 8)
        .map(|s| u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
}

/// Validate a minimal static ELF64 image and collect its `PT_LOAD` plan, WITHOUT allocating a slot (so a
/// rejection leaks nothing). `win_size` is the slot's user-window size — every segment's in-window span
/// (`p_vaddr - min_vaddr + p_memsz`) must fit it. Returns the plan or a `&'static str` naming the
/// rejection (an honest error, surfaced by the loader's log line and to the shell verb).
pub fn validate_elf(b: &[u8], win_size: usize) -> Result<ElfPlan, &'static str> {
    if b.len() < EHDR_SIZE {
        return Err("truncated ELF header");
    }
    // e_ident checks (magic already matched by `is_elf_image`).
    if b[4] != ELFCLASS64 {
        return Err("not ELF64 (EI_CLASS != 2)");
    }
    if b[5] != ELFDATA2LSB {
        return Err("not little-endian (EI_DATA != 1)");
    }
    if rd_u16(b, 16) != Some(ET_EXEC) {
        return Err("not ET_EXEC");
    }
    if rd_u16(b, 18) != Some(EM_X86_64) {
        return Err("not EM_X86_64");
    }
    let e_entry = rd_u64(b, 24).ok_or("bad e_entry")?;
    let e_phoff = rd_u64(b, 32).ok_or("bad e_phoff")? as usize;
    let e_phentsize = rd_u16(b, 54).ok_or("bad e_phentsize")? as usize;
    let e_phnum = rd_u16(b, 56).ok_or("bad e_phnum")? as usize;
    if e_phentsize != PHDR_SIZE {
        return Err("unexpected e_phentsize");
    }
    if e_phnum == 0 {
        return Err("no program headers");
    }
    // The whole program-header table must lie inside the image.
    let ph_end = e_phoff.checked_add(e_phnum * PHDR_SIZE).ok_or("phdr table overflow")?;
    if ph_end > b.len() {
        return Err("program-header table out of image");
    }

    let mut segs = [ElfSeg { off: 0, vaddr: 0, filesz: 0, memsz: 0, flags: 0 }; MAX_LOAD_SEGS];
    let mut nsegs = 0usize;
    let mut min_vaddr = u64::MAX;
    let mut stack_req = 0usize;
    for i in 0..e_phnum {
        let ph = e_phoff + i * PHDR_SIZE;
        let p_type = rd_u32(b, ph).ok_or("bad p_type")?;
        if p_type == PT_GNU_STACK {
            stack_req = rd_u64(b, ph + 40).ok_or("bad p_memsz")? as usize; // RING3WIN: the stack request
            continue;
        }
        if p_type != PT_LOAD {
            continue; // ignore PT_GNU_STACK / PT_PHDR / etc. — a minimal loader maps only PT_LOAD
        }
        let p_flags = rd_u32(b, ph + 4).ok_or("bad p_flags")?;
        let p_offset = rd_u64(b, ph + 8).ok_or("bad p_offset")? as usize;
        let p_vaddr = rd_u64(b, ph + 16).ok_or("bad p_vaddr")?;
        let p_filesz = rd_u64(b, ph + 32).ok_or("bad p_filesz")? as usize;
        let p_memsz = rd_u64(b, ph + 40).ok_or("bad p_memsz")? as usize;
        if nsegs >= MAX_LOAD_SEGS {
            return Err("too many PT_LOAD segments");
        }
        if p_filesz > p_memsz {
            return Err("p_filesz > p_memsz");
        }
        // The segment's file bytes must lie inside the image.
        let file_end = p_offset.checked_add(p_filesz).ok_or("segment file range overflow")?;
        if file_end > b.len() {
            return Err("segment file range out of image");
        }
        // W^X: a segment must not be both writable and executable (page shapes are RO+X or RW+NX).
        if p_flags & PF_W != 0 && p_flags & PF_X != 0 {
            return Err("W^X: segment both writable and executable");
        }
        segs[nsegs] =
            ElfSeg { off: p_offset, vaddr: p_vaddr, filesz: p_filesz, memsz: p_memsz, flags: p_flags };
        nsegs += 1;
        if p_vaddr < min_vaddr {
            min_vaddr = p_vaddr;
        }
    }
    if nsegs == 0 {
        return Err("no PT_LOAD segments");
    }
    if min_vaddr >= super::syscall::user_base() + memory::XWIN_OFF as u64 {
        return validate_elf_model(e_entry, min_vaddr, segs, nsegs, stack_req);
    }
    // Every segment must fit the slot's user window once biased so min_vaddr maps to the window base.
    let mut entry_in_exec = false;
    for i in 0..nsegs {
        let s = segs[i];
        let win_off = s.vaddr.checked_sub(min_vaddr).ok_or("vaddr below min")? as usize;
        let span_end = win_off.checked_add(s.memsz).ok_or("segment span overflow")?;
        if span_end > win_size {
            return Err("segment overflows the slot window");
        }
        // WINX-8 defence-in-depth — REFUSE an RX segment that crosses into the FINAL window page.
        //
        // The loader itself parks the initial ring-3 RSP at the window TOP ([`map_image_into_slot`]:
        // `sp = (base + size) & !0xF`), so the LAST page of the window is the first stack page of
        // EVERY program this loader starts — the program's first frame push lands in it. An
        // executable segment is mapped read-only (`protect_user_slot_range` clears the writable bit
        // on every page a non-`PF_W` segment covers, and the W^X check above already refused W+X),
        // so an executable memsz that spills into that page is loaded-to-die: the first stack write
        // takes `#PF err=0x7` and the fault net kills the program at RUNTIME with no load-time
        // diagnosis — the WINX-8 metal fault class (VUG's FILEHDR-layout RX memsz under a stack
        // carve). Refuse it HERE, with a witness, instead.
        //
        // Deliberately ONLY the final page — the one bound the LOADER itself imposes (its own `sp`
        // choice). Program-private stack carves lower in the window (user-vug parks worker stacks
        // at `base + 0x3000` / `base + 0x3800` — see `crates/user-vug/src/main.rs`) are a PROGRAM
        // contract, not loader law: a future image may legitimately claim page 2 as text and place
        // its stacks elsewhere, so those bounds are NOT enforced here. The boundary is exclusive at
        // the page edge: an RX segment ending exactly AT `win_size - 4 KiB` owns no byte of the
        // final page and loads.
        if s.flags & PF_X != 0 && s.memsz > 0 {
            // Derivations: `win_size` is the caller's `syscall::user_window_size()` (=
            // `USER_WINDOW_PAGES` * 4 KiB, documented "code, data, and two stack pages"); the page
            // size is `memory::PAGE_4K`, the same constant the flat path bounds against.
            let carve_off = win_size - memory::PAGE_4K as usize; // first byte of the final page
            if span_end > carve_off {
                serial_println!(
                    ":: elf: REFUSED rx-crosses-final-window-page seg=[{:#x},{:#x}) carve=[{:#x},{:#x}) ::",
                    s.vaddr,
                    s.vaddr + s.memsz as u64,
                    min_vaddr + carve_off as u64,
                    min_vaddr + win_size as u64
                );
                return Err("rx segment crosses the final window page (the initial stack page)");
            }
        }
        // The entry must land inside an EXECUTABLE segment's mapped range (a data-only entry would fault).
        if s.flags & PF_X != 0 && e_entry >= s.vaddr && e_entry < s.vaddr + s.memsz as u64 {
            entry_in_exec = true;
        }
    }
    if e_entry < min_vaddr {
        return Err("entry below the lowest segment");
    }
    if !entry_in_exec {
        return Err("entry not in an executable segment");
    }
    let fmap = stormfault_plan(&segs[..nsegs], min_vaddr, stack_req, false)?;
    Ok(ElfPlan { entry: e_entry, min_vaddr, segs, nsegs, model_elf: false, stack: 0, max_end: 0, fmap })
}

/// RING3WIN: the elf-model half of [`validate_elf`] — every segment (p_vaddr = absolute VA in the ELF
/// window) plus the stack and its guard page must fit the window; anything larger is refused `-ENOMEM`
/// with a line. Rebases every address to a window offset before returning.
fn validate_elf_model(
    e_entry: u64,
    min_vaddr: u64,
    mut segs: [ElfSeg; MAX_LOAD_SEGS],
    nsegs: usize,
    stack_req: usize,
) -> Result<ElfPlan, &'static str> {
    let base = super::syscall::user_base();
    let e_entry = e_entry.checked_sub(base).ok_or("entry below the window base")?;
    let min_vaddr = min_vaddr - base;
    for s in &mut segs[..nsegs] {
        s.vaddr -= base; // >= min_vaddr >= base, checked by the caller's branch
    }
    let top = (memory::XWIN_OFF + memory::XWIN_BYTES) as u64;
    let stack = if stack_req == 0 { una_abi::USER_STACK_DEFAULT as usize } else { (stack_req + 0xFFF) & !0xFFF };
    if stack > una_abi::USER_STACK_MAX as usize {
        serial_println!(":: RING3WIN: refused stack={} max={} -ENOMEM ::", stack, una_abi::USER_STACK_MAX);
        return Err("RING3WIN: declared stack exceeds the 1 MiB cap (-ENOMEM)");
    }
    let limit = top - stack as u64 - memory::PAGE_4K; // the guard page sits between image/heap and stack
    let mut max_end = 0u64;
    let mut entry_in_exec = false;
    for s in &segs[..nsegs] {
        let end = s.vaddr.checked_add(s.memsz as u64).ok_or("segment span overflow")?;
        if end > max_end {
            max_end = end;
        }
        if s.flags & PF_X != 0 && e_entry >= s.vaddr && e_entry < end {
            entry_in_exec = true;
        }
    }
    if max_end > limit {
        serial_println!(
            ":: RING3WIN: refused image span={} stack={} cap={} -ENOMEM ::",
            max_end - min_vaddr, stack, memory::XWIN_BYTES
        );
        return Err("RING3WIN: image + stack exceed the ELF window (USER_WINDOW_BYTES, -ENOMEM)");
    }
    if !entry_in_exec {
        return Err("entry not in an executable segment");
    }
    let fmap = stormfault_plan(&segs[..nsegs], 0, stack_req, true)?;
    Ok(ElfPlan { entry: e_entry, min_vaddr, segs, nsegs, model_elf: true, stack, max_end, fmap })
}

/// The result of [`map_image_into_slot`]: the ring-3 run parameters.
pub struct Mapped {
    /// The ring-3 ENTRY VA (window base for a flat blob, `bias + e_entry` for an ELF).
    pub entry: u64,
    /// 16-aligned window top = the initial ring-3 RSP.
    pub sp: u64,
    /// The slot's CR3 (its PML4 physical base).
    pub cr3: u64,
    pub slot: usize,
    pub len: usize,
    pub is_elf: bool,
    pub nsegs: u32,
}

/// A mapping failure, rendered as an operator string by the callers.
pub enum MapErr {
    Empty,
    /// A flat blob larger than one code page.
    BadSize(usize),
    BadElf(&'static str),
    NoSlot,
}

impl MapErr {
    /// The operator-facing rendering (the shell prints this after `run:`/`bg:`).
    pub fn as_str(&self) -> &'static str {
        match self {
            MapErr::Empty => "empty image",
            MapErr::BadSize(_) => "flat blob larger than one code page",
            MapErr::BadElf(why) => why,
            MapErr::NoSlot => "no free address-space slot",
        }
    }
}

/// WINX-2: the image MAPPER — the x86 twin of aarch64's `map_image_into_slot`.
///
/// Given the WHOLE read image (already bounded to the user window by the caller), it dispatches on the
/// ELF magic, VALIDATES fully BEFORE allocating a slot, then copies each `PT_LOAD` (or the one flat page)
/// into a FRESH slot and applies per-segment W^X page permissions.
///
/// # Two x86 divergences from the aarch64 twin, both simplifications
/// * **No I-cache maintenance.** aarch64 must `icache_sync_range` each executable segment after the copy
///   because its I- and D-caches are not coherent. x86 instruction caches are coherent with stores by
///   architecture (a self-modifying-code store is observed by the fetcher; only cross-modifying code
///   needs a serialising event, and the `iretq` into ring 3 is one), so there is nothing to sync.
/// * **No break-before-make.** The aarch64 mapper must BBM its leaf permission changes. x86 permits a
///   live permission change on a present leaf followed by `invlpg`, which is what
///   `memory::protect_user_slot_range` does.
///
/// Copies go through the KERNEL identity alias (`slot_backing_ptr`), never through the ring-3 VA, so the
/// code pages are never writable through their ring-3 mapping and W^X holds by construction — the same
/// discipline `build_slot` documents for the U1a blob.
pub fn map_image_into_slot(bytes: &[u8]) -> Result<Mapped, MapErr> {
    if bytes.is_empty() {
        return Err(MapErr::Empty);
    }
    let base = super::syscall::user_base();
    let size = super::syscall::user_window_size();
    let elf_plan = if is_elf_image(bytes) {
        let plan = validate_elf(bytes, size).map_err(MapErr::BadElf)?;
        if plan.model_elf {
            return map_elf_model(bytes, &plan); // RING3WIN: the image asked for the ELF window
        }
        Some(plan)
    } else {
        // FLAT path: the historical U2 model — one code page, entered at offset 0,
        // position-independent. Keep the exact one-page bound.
        if bytes.len() > memory::PAGE_4K as usize {
            return Err(MapErr::BadSize(bytes.len()));
        }
        None
    };

    // Slot allocated LAST: no fallible step may follow, so a rejection above leaks nothing.
    let slot = memory::alloc_user_space().ok_or(MapErr::NoSlot)?;
    let backing = memory::slot_backing_ptr(slot);
    let (entry, nsegs, is_elf) = match &elf_plan {
        Some(plan) => {
            // Scrub the whole program window first: `build_slot` does not zero the backing, and a
            // recycled slot must not leak the previous tenant's bytes into the gaps BETWEEN this
            // image's segments (or into its stack). Each segment's own memsz is zeroed again below,
            // which is redundant but keeps the per-segment `.bss` contract local and obvious.
            unsafe { core::ptr::write_bytes(backing, 0, size) };
            for i in 0..plan.nsegs {
                let s = plan.segs[i];
                let dst_off = (s.vaddr - plan.min_vaddr) as usize;
                unsafe {
                    let dst = backing.add(dst_off);
                    core::ptr::write_bytes(dst, 0, s.memsz); // zero the whole memsz (covers the bss tail)
                    core::ptr::copy_nonoverlapping(bytes.as_ptr().add(s.off), dst, s.filesz);
                }
            }
            // Apply per-segment page permissions AFTER every copy (the copies went through the identity
            // alias, so the ring-3 leaves were never writable for code in the first place).
            for i in 0..plan.nsegs {
                let s = plan.segs[i];
                let dst_off = (s.vaddr - plan.min_vaddr) as usize;
                let exec = s.flags & PF_X != 0;
                let writable = s.flags & PF_W != 0;
                unsafe { memory::protect_user_slot_range(slot, dst_off, s.memsz, writable, exec) };
            }
            let entry = base + (plan.entry - plan.min_vaddr);
            (entry, plan.nsegs as u32, true)
        }
        None => {
            // FLAT: copy to the code page at the window base. `build_slot` already left page 0 as
            // USER + RX + read-only, which is exactly the flat contract, so no re-protect is needed.
            unsafe {
                core::ptr::write_bytes(backing, 0, size);
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), backing, bytes.len());
            }
            (base, 1, false)
        }
    };
    seg_map_set(slot, elf_plan.as_ref().map_or(elf_core::FaultMap::NONE, |p| p.fmap)); // STORMFAULT: flat = no map
    // RING3WIN: a fixed-model program's heap is the whole ELF window (its stack stays in the classic one).
    unsafe { memory::xwin_free(slot) };
    memory::xwin_set_heap(slot, memory::XWIN_OFF as u64, (memory::XWIN_OFF + memory::XWIN_BYTES) as u64);
    Ok(Mapped {
        entry,
        sp: (base + size as u64) & !0xF, // 16-aligned window top = the initial ring-3 RSP
        cr3: memory::slot_cr3(slot),
        slot,
        len: bytes.len(),
        is_elf,
        nsegs,
    })
}

/// RING3WIN: place an elf-model image (validated) into a fresh slot's ELF window: every PT_LOAD page
/// mapped from the heap with its segment's rights (shared pages take the union; W+X refused), the file
/// bytes copied through the identity alias (frames arrive zeroed, so the BSS tail is zero), the stack
/// mapped eagerly at the window top with an unmapped guard page beneath, the heap armed from the page after
/// the highest segment to the guard. Any failure releases the slot through the ordinary teardown.
fn map_elf_model(bytes: &[u8], plan: &ElfPlan) -> Result<Mapped, MapErr> {
    let base = super::syscall::user_base();
    let slot = memory::alloc_user_space().ok_or(MapErr::NoSlot)?;
    unsafe {
        memory::xwin_free(slot);
        // The classic window is mapped by `build_slot` regardless; scrub it so a recycled slot leaks nothing.
        core::ptr::write_bytes(memory::slot_backing_ptr(slot), 0, super::syscall::user_window_size());
    }
    let fail = |why: &'static str| -> Result<Mapped, MapErr> {
        serial_println!(":: RING3WIN: refused slot={} reason={} -ENOMEM ::", slot, why);
        crate::arch::memory::free_user_space_by_cr3(memory::slot_cr3(slot));
        Err(MapErr::BadElf(why))
    };
    let pg = memory::PAGE_4K;
    for s in &plan.segs[..plan.nsegs] {
        let (w, x) = (s.flags & PF_W != 0, s.flags & PF_X != 0);
        let mut p = s.vaddr & !(pg - 1);
        let end = (s.vaddr + s.memsz as u64 + pg - 1) & !(pg - 1);
        while p < end {
            if unsafe { memory::xwin_map_page(slot, p as usize, w, x) }.is_err() {
                return fail("RING3WIN: out of frames, or segments share a page with conflicting W/X");
            }
            p += pg;
        }
    }
    for s in &plan.segs[..plan.nsegs] {
        if !memory::xwin_copy_in(slot, s.vaddr as usize, &bytes[s.off..s.off + s.filesz]) {
            return fail("RING3WIN: segment copy hit an unmapped page");
        }
    }
    let top = (memory::XWIN_OFF + memory::XWIN_BYTES) as u64;
    let stack_lo = top - plan.stack as u64;
    let mut p = stack_lo;
    while p < top {
        if unsafe { memory::xwin_map_page(slot, p as usize, true, false) }.is_err() {
            return fail("RING3WIN: out of frames mapping the stack");
        }
        p += pg;
    }
    let heap_lo = (plan.max_end + pg - 1) & !(pg - 1);
    memory::xwin_set_heap(slot, heap_lo, stack_lo - pg);
    seg_map_set(slot, plan.fmap); // STORMFAULT
    serial_println!(
        ":: RING3WIN: model=elf slot={} segs={} span={} stack={} heap=[{:#x},{:#x}) frames={} ::",
        slot, plan.nsegs, plan.max_end - plan.min_vaddr, plan.stack, base + heap_lo, base + stack_lo - pg,
        memory::xwin_live_pages()
    );
    Ok(Mapped {
        entry: base + plan.entry,
        sp: (base + top) & !0xF,
        cr3: memory::slot_cr3(slot),
        slot,
        len: bytes.len(),
        is_elf: true,
        nsegs: plan.nsegs as u32,
    })
}

// EXECNAME (rmbp-ledger B322, R82): the program's launch declaration, for the x86 loader's callers.
// The note walk itself is `midden_core::app_note_flags` — the shell core both rings link, so the
// aarch64 bare-name launch reads the same record the same way and the host tests it — and this is the
// x86 loader's door onto it: an image this loader would refuse by magic or machine declares nothing.
/// The `una_abi::APP_FLAG_*` bits `bytes` declares in its `.note.unaos.app` note; 0 when it carries no
/// note, is not ELF, or is not EM_X86_64 (0 = the foreground fallback).
pub fn app_flags(bytes: &[u8]) -> u32 {
    if !is_elf_image(bytes) || rd_u16(bytes, 18) != Some(EM_X86_64) {
        return 0;
    }
    midden_core::app_note_flags(bytes).unwrap_or(0)
}
// The core carries its own copies (it takes no dependencies); they ARE the ABI's, or this does not build.
const _: () = assert!(
    midden_core::APP_NOTE_TYPE == una_abi::APP_NOTE_TYPE
        && midden_core::APP_FLAG_WINDOWED == una_abi::APP_FLAG_WINDOWED
        && midden_core::APP_FLAG_RESIDENT == una_abi::APP_FLAG_RESIDENT
        && midden_core::APP_NOTE_NAME.len() == una_abi::APP_NOTE_NAME.len()
);

// =================================================================================================
// STORMFAULT (rmbp-ledger B351): the segment plan and the fault line's `seg=` word.
//
// Both models already map every PT_LOAD's full p_memsz zero-filled (the fixed model scrubs the classic
// window and zeroes each memsz; the elf model maps every page of [vaddr, vaddr+memsz) from zeroed heap
// frames) and the elf model honours PT_GNU_STACK. What this adds, through `elf_core::plan` (the shared
// core the host tests run over VUG.ELF's and LUMEN.ELF's real headers): the refusals the per-model checks
// above do not make — segments overlapping each other, a segment or stack over the args page, a fixed-model
// PT_GNU_STACK that would land on the bss — each a NAMED error and a `:: STORMFAULT: refused` line instead of
// a ring-3 fault; and the per-slot fault map, so a ring-3 #PF names what it hit. Design + witness:
// docs/dev/evidence/rmbp-1005/STORMFAULT.md.
// =================================================================================================

use core::sync::atomic::{AtomicU64, Ordering};

// The core restates these (it takes no dependencies); they ARE the ABI's, or this does not build.
const _: () = assert!(elf_core::PT_GNU_STACK == PT_GNU_STACK && elf_core::PF_W == PF_W && elf_core::PF_X == PF_X && elf_core::MAX_SEGS == MAX_LOAD_SEGS);

/// Run the shared plan over validated segments. `rebase` is subtracted from each vaddr (the fixed model's
/// min_vaddr; 0 for the elf model, whose segments are already window offsets).
fn stormfault_plan(segs: &[ElfSeg], rebase: u64, stack_req: usize, elf: bool) -> Result<elf_core::FaultMap, &'static str> {
    let mut v = [elf_core::Seg::default(); MAX_LOAD_SEGS];
    for (d, s) in v.iter_mut().zip(segs) {
        *d = elf_core::Seg { vaddr: s.vaddr - rebase, filesz: s.filesz as u64, memsz: s.memsz as u64, flags: s.flags };
    }
    let w = elf_core::Window {
        lo: if elf { memory::XWIN_OFF as u64 } else { 0 },
        hi: if elf { (memory::XWIN_OFF + memory::XWIN_BYTES) as u64 } else { super::syscall::user_window_size() as u64 },
        args: elf_core::Range { lo: una_abi::USER_ARGS_OFF, hi: una_abi::USER_ARGS_OFF + una_abi::USER_ARGS_BYTES as u64 },
        elf,
        refuse_overlap: elf,
        stack_default: una_abi::USER_STACK_DEFAULT,
        stack_max: una_abi::USER_STACK_MAX,
    };
    match elf_core::plan(&v[..segs.len()], stack_req as u64, &w) {
        Ok(p) => {
            static LINES: AtomicU64 = AtomicU64::new(0);
            if LINES.fetch_add(1, Ordering::Relaxed) < 32 {
                serial_println!(
                    ":: STORMFAULT: plan model={} segs={} bss={} stack=[{:#x},{:#x}) guard={} ::",
                    if elf { "elf" } else { "fixed" }, segs.len(), p.bss_bytes(), p.stack.lo, p.stack.hi, !p.guard.is_empty()
                );
            }
            Ok(p.fault_map())
        }
        Err(e) => {
            serial_println!(":: STORMFAULT: refused model={} reason={} ::", if elf { "elf" } else { "fixed" }, e.word());
            Err(e.as_str())
        }
    }
}

const SEGMAP_WORDS: usize = 6;
#[allow(clippy::declare_interior_mutable_const)]
const Z: AtomicU64 = AtomicU64::new(0);
#[allow(clippy::declare_interior_mutable_const)]
const ZROW: [AtomicU64; SEGMAP_WORDS] = [Z; SEGMAP_WORDS];
/// Per slot: bss0 lo/hi, bss1 lo/hi, stack lo/hi (window offsets). All-zero = nothing to name.
static SEGMAP: [[AtomicU64; SEGMAP_WORDS]; memory::USER_SLOTS] = [ZROW; memory::USER_SLOTS];

fn seg_map_set(slot: usize, m: elf_core::FaultMap) {
    let Some(row) = SEGMAP.get(slot) else { return };
    let w = [m.bss[0].lo, m.bss[0].hi, m.bss[1].lo, m.bss[1].hi, m.stack.lo, m.stack.hi];
    for (a, v) in row.iter().zip(w) {
        a.store(v, Ordering::Release);
    }
}

/// STORMFAULT: forget a slot's map (its teardown; a fixture that builds a slot by hand names nothing).
pub fn seg_map_clear(slot: usize) {
    seg_map_set(slot, elf_core::FaultMap::NONE);
}

/// STORMFAULT: the `seg=` word for a ring-3 fault at `cr2` in the CURRENT slot: `bss`, `stack` (guard
/// included), or `none`. Lock-free (the fault handler's context): reads the live CR3's slot and six atomics.
pub fn fault_seg(cr2: u64) -> &'static str {
    let Some(slot) = memory::current_slot() else { return "none" };
    let Some(off) = cr2.checked_sub(super::syscall::user_base()) else { return "none" };
    let r = &SEGMAP[slot];
    let g = |i: usize| r[i].load(Ordering::Acquire);
    let m = elf_core::FaultMap {
        bss: [elf_core::Range { lo: g(0), hi: g(1) }, elf_core::Range { lo: g(2), hi: g(3) }],
        stack: elf_core::Range { lo: g(4), hi: g(5) },
    };
    elf_core::classify(&m, off).as_str()
}

/// STORMFAULT: the `tests elfbss` image — an elf-model ELF64 built in place (no staging): a text page at
/// the ELF window base, a 64 KiB `.bss` (filesz 0, memsz 64 KiB) one page above it, and PT_GNU_STACK 64 KiB.
/// The program writes 0x5A to the LAST bss byte, reads it back, reads the first and middle bss bytes (zero
/// iff the loader zero-filled), and exits with `first | middle | (last ^ 0x5A)` — 0 is the only pass.
fn elfbss_image(bss: u64) -> alloc::vec::Vec<u8> {
    const CODE_OFF: usize = 0x100;
    let xwin = una_abi::USER_XWIN_VA_X86;
    let (first, mid, last) = (xwin + 0x1000, xwin + 0x1000 + bss / 2, xwin + 0x1000 + bss - 1);
    let mut code: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    code.extend_from_slice(&[0x48, 0xB8]); // movabs rax, last
    code.extend_from_slice(&last.to_le_bytes());
    code.extend_from_slice(&[0xC6, 0x00, 0x5A]); // mov byte [rax], 0x5A
    code.extend_from_slice(&[0x48, 0xB9]); // movabs rcx, first
    code.extend_from_slice(&first.to_le_bytes());
    code.extend_from_slice(&[0x0F, 0xB6, 0x39]); // movzx edi, byte [rcx]
    code.extend_from_slice(&[0x48, 0xB9]); // movabs rcx, mid
    code.extend_from_slice(&mid.to_le_bytes());
    code.extend_from_slice(&[0x0F, 0xB6, 0x11]); // movzx edx, byte [rcx]
    code.extend_from_slice(&[0x09, 0xD7]); // or edi, edx
    code.extend_from_slice(&[0x0F, 0xB6, 0x10]); // movzx edx, byte [rax]
    code.extend_from_slice(&[0x83, 0xF2, 0x5A]); // xor edx, 0x5A
    code.extend_from_slice(&[0x09, 0xD7]); // or edi, edx
    code.extend_from_slice(&[0xB8]); // mov eax, SYS_EXIT
    code.extend_from_slice(&(una_abi::SYS_EXIT as u32).to_le_bytes());
    code.extend_from_slice(&[0x0F, 0x05, 0x0F, 0x0B]); // syscall; ud2
    let tlen = CODE_OFF + code.len();
    let mut img = alloc::vec![0u8; tlen];
    img[0..4].copy_from_slice(&ELF_MAGIC);
    img[4] = ELFCLASS64;
    img[5] = ELFDATA2LSB;
    img[6] = 1;
    img[16..18].copy_from_slice(&ET_EXEC.to_le_bytes());
    img[18..20].copy_from_slice(&EM_X86_64.to_le_bytes());
    img[20..24].copy_from_slice(&1u32.to_le_bytes());
    img[24..32].copy_from_slice(&(xwin + CODE_OFF as u64).to_le_bytes());
    img[32..40].copy_from_slice(&(EHDR_SIZE as u64).to_le_bytes());
    img[52..54].copy_from_slice(&(EHDR_SIZE as u16).to_le_bytes());
    img[54..56].copy_from_slice(&(PHDR_SIZE as u16).to_le_bytes());
    img[56..58].copy_from_slice(&3u16.to_le_bytes());
    let mut ph = |i: usize, t: u32, f: u32, va: u64, fs: u64, ms: u64| {
        let p = EHDR_SIZE + i * PHDR_SIZE;
        img[p..p + 4].copy_from_slice(&t.to_le_bytes());
        img[p + 4..p + 8].copy_from_slice(&f.to_le_bytes());
        img[p + 16..p + 24].copy_from_slice(&va.to_le_bytes());
        img[p + 24..p + 32].copy_from_slice(&va.to_le_bytes());
        img[p + 32..p + 40].copy_from_slice(&fs.to_le_bytes());
        img[p + 40..p + 48].copy_from_slice(&ms.to_le_bytes());
        img[p + 48..p + 56].copy_from_slice(&0x1000u64.to_le_bytes());
    };
    ph(0, PT_LOAD, 5, xwin, tlen as u64, tlen as u64);
    ph(1, PT_LOAD, 6, xwin + 0x1000, 0, bss);
    ph(2, PT_GNU_STACK, 6, 0, 0, 64 << 10);
    img[CODE_OFF..].copy_from_slice(&code);
    img
}

/// STORMFAULT `tests elfbss` (rmbp-ledger B351): the 64 KiB-bss program runs to `exit 0` through the real
/// launcher; its plan names the last bss byte `bss`; a window-leaving bss and a fixed-model PT_GNU_STACK on
/// top of the bss are refused by name. Verdict:
/// `:: STORMFAULT: elfbss bss=65536 exit=<n|fault|timeout> seg_last=<..> refuse_window=<0|1> refuse_stack=<0|1> -> PASS|FAIL ::`.
pub fn elfbss_selftest() {
    const BSS: u64 = 64 << 10;
    let img = elfbss_image(BSS);
    let seg_last = match validate_elf(&img, super::syscall::user_window_size()) {
        Ok(p) => elf_core::classify(&p.fmap, memory::XWIN_OFF as u64 + 0x1000 + BSS - 1).as_str(),
        Err(e) => {
            serial_println!(":: STORMFAULT: elfbss image refused: {} -> FAIL ::", e);
            return;
        }
    };
    let exit = match super::syscall::run_user_image_argv("elfbss", &img, 2000, &["elfbss"]) {
        Ok((super::syscall::RunOutcome::Exited(s), _)) => Some(s),
        Ok((super::syscall::RunOutcome::Faulted, _)) => { serial_println!("[elfbss] run: fault"); None }
        Ok((super::syscall::RunOutcome::Timeout, _)) => { serial_println!("[elfbss] run: timeout"); None }
        Err(e) => { serial_println!("[elfbss] run refused: {}", e); None }
    };
    // A bss that runs past the window's end.
    let refuse_window = validate_elf(&elfbss_image(memory::XWIN_BYTES as u64), super::syscall::user_window_size()).is_err();
    // A fixed-model image (linked at 0) whose PT_GNU_STACK would cover its own bss: text page, bss
    // [0x1000, 0x3800), stack request 0x1000 -> sp_lo 0x3000 < 0x3800.
    let mut fixed = elfbss_image(0x2800);
    let xwin = una_abi::USER_XWIN_VA_X86;
    for i in 0..3 {
        let p = EHDR_SIZE + i * PHDR_SIZE + 16;
        let va = u64::from_le_bytes(fixed[p..p + 8].try_into().unwrap_or([0; 8]));
        if va >= xwin {
            fixed[p..p + 8].copy_from_slice(&(va - xwin).to_le_bytes());
        }
    }
    let e = u64::from_le_bytes(fixed[24..32].try_into().unwrap_or([0; 8])) - xwin;
    fixed[24..32].copy_from_slice(&e.to_le_bytes());
    let gs = EHDR_SIZE + 2 * PHDR_SIZE + 40;
    fixed[gs..gs + 8].copy_from_slice(&0x1000u64.to_le_bytes());
    let refuse_stack = matches!(validate_elf(&fixed, super::syscall::user_window_size()), Err(m) if m == elf_core::PlanErr::StackOverlapsImage.as_str());
    let pass = exit == Some(0) && seg_last == "bss" && refuse_window && refuse_stack;
    let ex: alloc::string::String = match exit { Some(s) => alloc::format!("{}", s), None => "fault-or-timeout".into() };
    serial_println!(
        ":: STORMFAULT: elfbss bss={} exit={} seg_last={} refuse_window={} refuse_stack={} -> {} ::",
        BSS, ex, seg_last, refuse_window as u8, refuse_stack as u8, if pass { "PASS" } else { "FAIL" }
    );
}
