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
    Ok(ElfPlan { entry: e_entry, min_vaddr, segs, nsegs, model_elf: false, stack: 0, max_end: 0 })
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
        return Err("RING3WIN: image + stack exceed the 4 MiB ELF window (-ENOMEM)");
    }
    if !entry_in_exec {
        return Err("entry not in an executable segment");
    }
    Ok(ElfPlan { entry: e_entry, min_vaddr, segs, nsegs, model_elf: true, stack, max_end })
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
