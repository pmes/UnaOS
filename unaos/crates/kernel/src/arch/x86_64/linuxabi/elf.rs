// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! LINUXABI M1 — static Linux x86_64 ELF: parse, map at the FIXED vaddrs, build the System V stack.
//!
//! Accepts `ET_EXEC` / `EM_X86_64` with no `PT_INTERP` (static, non-PIE — what musl-static busybox and a static
//! toolchain are) and, since WINDOW2 (B361), `ET_DYN` with no `PT_INTERP` = a STATIC-PIE (LLD.LNX, R85 "relinked
//! as PIE"): it is placed at [`PIE_BASE`] in the process's private PML4[2] half and relocates ITSELF (glibc's
//! `_dl_relocate_static_pie`, musl's `_dlstart`) — the kernel applies no relocation. Everything read from the
//! image is bounds-checked; W+X segments are refused.

use super::{AddrSpace, PAGE, STACK_PAGES, STACK_TOP};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

const PT_LOAD: u32 = 1;
const PT_INTERP: u32 = 3;
const MAX_SEGS: usize = 16;
/// The fixed-address (ET_EXEC) window: the process's view of low memory REPLACES the kernel's identity
/// mapping under its CR3, so these pages must be free RAM that the kernel never touches through the identity
/// map while the process runs. WINDOW2 (B361): raised 16 MiB -> the ring-3 window span (64 MiB); the load
/// checks the range is Usable RAM AND outside the kernel heap ([`low_window_ok`]), and the user frame pool
/// (`vm.rs`) starts at this limit instead of 16 MiB.
pub const IMAGE_LIMIT: u64 = una_abi::USER_WINDOW_BYTES;
const IMAGE_FLOOR: u64 = 0x1_0000;
/// WINDOW2: where a static-PIE is placed — the top 4 GiB of the process's private PML4[2] half, above the
/// second mmap window (`vm::MMAP2_LIMIT`) and never inherited from the kernel, so no RAM check applies.
pub const PIE_BASE: u64 = 0x0000_017F_0000_0000;
/// WINDOW2: the largest static-PIE span (PT_LOAD lowest page .. highest end).
pub const PIE_SPAN: u64 = 1 << 30;
const _: () = assert!(PIE_BASE >= super::vm::MMAP2_LIMIT && PIE_BASE + PIE_SPAN <= 0x0000_0180_0000_0000);

/// WINDOW2: may a fixed-address image occupy `[lo, lo+len)` of the low window? Usable RAM per the firmware map
/// and clear of the kernel heap (a heap below 64 MiB — a small-RAM machine — refuses instead of overlaying it).
pub fn low_window_ok(lo: u64, len: u64) -> bool {
    let (hs, hl) = super::memory::selfbuild3_heap_window();
    let hi = lo.saturating_add(len);
    super::memory::region_is_usable(lo, len) && (hl == 0 || hi <= hs || lo >= hs + hl as u64)
}
/// WINDOW2: is this plan a static-PIE placed at [`PIE_BASE`] (its pages are not low-window RAM)?
pub fn is_pie(plan: &Plan) -> bool {
    plan.pie
}

#[derive(Clone, Copy)]
pub struct Seg {
    pub off: u64,
    pub vaddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub flags: u32,
}

pub struct Plan {
    pub entry: u64,
    pub segs: Vec<Seg>,
    pub phdr_va: u64,
    pub phnum: u64,
    pub phent: u64,
    /// WINDOW2: a static-PIE (ET_DYN) — every vaddr above is already biased to [`PIE_BASE`].
    pub pie: bool,
}

fn u16_at(b: &[u8], o: usize) -> Option<u64> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as u64)
}
fn u32_at(b: &[u8], o: usize) -> Option<u64> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as u64)
}
fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    b.get(o..o + 8).map(|s| u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
}

pub fn parse(b: &[u8]) -> Result<Plan, &'static str> {
    parse_sized(b, b.len() as u64)
}

/// SELFBUILD4: parse from the image's HEAD (`b` holds at least the ELF header and the program-header table) for a file of
/// `flen` bytes — segment file ranges are checked against `flen`, so a lazy execve never reads the whole file.
pub fn parse_sized(b: &[u8], flen: u64) -> Result<Plan, &'static str> {
    if b.len() < 64 || b[0..4] != [0x7F, b'E', b'L', b'F'] {
        return Err("not an ELF image");
    }
    if b[4] != 2 || b[5] != 1 {
        return Err("not ELF64 little-endian");
    }
    let pie = match u16_at(b, 16) {
        Some(2) => false,
        Some(3) => true, // WINDOW2: ET_DYN without PT_INTERP = static-PIE (a shared object has no entry in an X segment: refused below)
        _ => return Err("not ET_EXEC or a static-PIE ET_DYN"),
    };
    if u16_at(b, 18) != Some(62) {
        return Err("not EM_X86_64");
    }
    let entry = u64_at(b, 24).ok_or("bad e_entry")?;
    let phoff = u64_at(b, 32).ok_or("bad e_phoff")?;
    let phent = u16_at(b, 54).ok_or("bad e_phentsize")?;
    let phnum = u16_at(b, 56).ok_or("bad e_phnum")?;
    if phent != 56 || phnum == 0 {
        return Err("bad program-header table");
    }
    let tab_end = phoff.checked_add(phnum * 56).ok_or("phdr overflow")?;
    if tab_end > b.len() as u64 {
        return Err("program-header table outside the image");
    }
    let mut segs = Vec::new();
    let mut total = 0u64;
    for i in 0..phnum as usize {
        let ph = phoff as usize + i * 56;
        let ty = u32_at(b, ph).ok_or("bad p_type")?;
        if ty == PT_INTERP as u64 {
            return Err("dynamic executable (PT_INTERP) — only static binaries are supported");
        }
        if ty != PT_LOAD as u64 {
            continue;
        }
        let flags = u32_at(b, ph + 4).ok_or("bad p_flags")? as u32;
        let off = u64_at(b, ph + 8).ok_or("bad p_offset")?;
        let vaddr = u64_at(b, ph + 16).ok_or("bad p_vaddr")?;
        let filesz = u64_at(b, ph + 32).ok_or("bad p_filesz")?;
        let memsz = u64_at(b, ph + 40).ok_or("bad p_memsz")?;
        if segs.len() >= MAX_SEGS {
            return Err("too many PT_LOAD segments");
        }
        if filesz > memsz || off.checked_add(filesz).map_or(true, |e| e > flen) {
            return Err("segment file range outside the image");
        }
        let end = vaddr.checked_add(memsz).ok_or("segment vaddr overflow")?;
        if !pie && (vaddr < IMAGE_FLOOR || end > IMAGE_LIMIT) {
            return Err("segment outside the loadable window (0x10000..64 MiB)");
        }
        if pie && end > PIE_SPAN {
            return Err("static-PIE segment past the PIE span (1 GiB)");
        }
        if flags & 2 != 0 && flags & 1 != 0 {
            return Err("W+X segment refused (W^X)");
        }
        total += memsz;
        segs.push(Seg { off, vaddr, filesz, memsz, flags });
    }
    if segs.is_empty() {
        return Err("no PT_LOAD segments");
    }
    if total > if pie { PIE_SPAN } else { IMAGE_LIMIT } {
        return Err("image too large");
    }
    if pie {
        // WINDOW2: place it. Link-time vaddrs start at 0 (or the lowest PT_LOAD page); bias every address the
        // loader, the stack builder (AT_PHDR, AT_ENTRY) and the lazy exec use, so downstream sees absolute VAs.
        let lo = segs.iter().map(|s| s.vaddr & !0xFFF).min().unwrap_or(0);
        let bias = PIE_BASE - lo;
        for s in segs.iter_mut() {
            s.vaddr += bias;
        }
        let entry = entry.checked_add(bias).ok_or("bad e_entry")?;
        return finish(entry, segs, phoff, phnum, phent, true);
    }
    finish(entry, segs, phoff, phnum, phent, false)
}

fn finish(entry: u64, segs: Vec<Seg>, phoff: u64, phnum: u64, phent: u64, pie: bool) -> Result<Plan, &'static str> {
    if !segs.iter().any(|s| entry >= s.vaddr && entry < s.vaddr + s.memsz && s.flags & 1 != 0) {
        return Err("entry point not in an executable segment");
    }
    // Where the program headers sit in memory (AT_PHDR): inside the segment that file-maps them.
    let mut phdr_va = 0;
    for s in &segs {
        if phoff >= s.off && phoff < s.off + s.filesz {
            phdr_va = s.vaddr + (phoff - s.off);
            break;
        }
    }
    Ok(Plan { entry, segs, phdr_va, phnum, phent, pie })
}

/// Map every segment (page permissions = union of the segments that share the page) and copy the file
/// bytes in. Frames arrive zeroed, so `.bss` and partial-page tails are already zero.
pub fn load(asp: &mut AddrSpace, img: &[u8], plan: &Plan) -> Result<(), &'static str> {
    let mut pages: BTreeMap<u64, (bool, bool)> = BTreeMap::new();
    for s in &plan.segs {
        let (w, x) = (s.flags & 2 != 0, s.flags & 1 != 0);
        let mut a = s.vaddr & !(PAGE - 1);
        let end = (s.vaddr + s.memsz + PAGE - 1) & !(PAGE - 1);
        while a < end {
            let e = pages.entry(a).or_insert((false, false));
            e.0 |= w;
            e.1 |= x;
            a += PAGE;
        }
    }
    let (lo, hi) = (*pages.keys().next().unwrap(), *pages.keys().next_back().unwrap() + PAGE);
    if !plan.pie && !low_window_ok(lo, hi - lo) {
        return Err("load range is not free RAM on this machine (would overlay the kernel or its heap)");
    }
    for (&va, &(w, x)) in &pages {
        if w && x {
            return Err("segments share a page with conflicting W/X");
        }
        if !asp.map_new(va, w, x) {
            return Err("out of pages mapping the image");
        }
    }
    for s in &plan.segs {
        let data = &img[s.off as usize..(s.off + s.filesz) as usize];
        if !asp.copy_out(s.vaddr, data, true) {
            return Err("segment copy failed");
        }
    }
    Ok(())
}

/// Map the stack and build argc/argv/envp/auxv per the System V x86_64 ABI; returns the entry rsp
/// (16-byte aligned, pointing at argc).
pub fn build_stack(
    asp: &mut AddrSpace,
    plan: &Plan,
    execfn: &str,
    argv: &[&str],
    envp: &[&str],
) -> Result<u64, &'static str> {
    let base = STACK_TOP - STACK_PAGES * PAGE;
    for i in 0..STACK_PAGES {
        if !asp.map_new(base + i * PAGE, true, false) {
            return Err("out of pages mapping the stack");
        }
    }
    let mut cur = STACK_TOP - 16;
    let mut put = |asp: &AddrSpace, data: &[u8]| -> Result<u64, &'static str> {
        cur -= data.len() as u64;
        if cur < base + 4096 || !asp.copy_out(cur, data, true) {
            return Err("stack overflow building the initial frame");
        }
        Ok(cur)
    };
    // AT_RANDOM: 16 bytes seeded from the TSC (not cryptographic — a stack-protector canary seed).
    let mut rnd = [0u8; 16];
    let t = crate::arch::now_cycles();
    for (i, b) in rnd.iter_mut().enumerate() {
        *b = (t.rotate_left((i as u32) * 7).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56) as u8;
    }
    let random_p = put(asp, &rnd)?;
    let mut s = Vec::from(execfn.as_bytes());
    s.push(0);
    let execfn_p = put(asp, &s)?;
    let platform_p = put(asp, b"x86_64\0")?; // LINUXABI3: AT_PLATFORM
    let mut env_p = Vec::new();
    for e in envp.iter().rev() {
        let mut s = Vec::from(e.as_bytes());
        s.push(0);
        env_p.push(put(asp, &s)?);
    }
    env_p.reverse();
    let mut arg_p = Vec::new();
    for a in argv.iter().rev() {
        let mut s = Vec::from(a.as_bytes());
        s.push(0);
        arg_p.push(put(asp, &s)?);
    }
    arg_p.reverse();
    let aux: [(u64, u64); 19] = [
        (3, plan.phdr_va),
        (4, plan.phent),
        (5, plan.phnum),
        (6, PAGE),
        (7, 0),
        (8, 0),
        (9, plan.entry),
        (11, 0),
        (12, 0),
        (13, 0),
        (14, 0),
        (17, 100),
        (23, 0),
        (25, random_p),
        (31, execfn_p),
        (16, super::fpu::hwcap()), // LINUXABI3: AT_HWCAP = CPUID.1:EDX (FXSR/SSE/SSE2 = bits 24/25/26)
        (26, 0),                   // AT_HWCAP2: no ring3mwait/fsgsbase advertised
        (15, platform_p),          // AT_PLATFORM "x86_64"
        (0, 0),
    ];
    let mut words: Vec<u64> = Vec::new();
    words.push(arg_p.len() as u64);
    words.extend_from_slice(&arg_p);
    words.push(0);
    words.extend_from_slice(&env_p);
    words.push(0);
    for (k, v) in aux {
        words.push(k);
        words.push(v);
    }
    let bytes = words.len() as u64 * 8;
    let sp = (cur - bytes) & !15;
    if sp < base + 4096 {
        return Err("stack overflow building the initial frame");
    }
    let mut buf = Vec::with_capacity(words.len() * 8);
    for w in &words {
        buf.extend_from_slice(&w.to_le_bytes());
    }
    if !asp.copy_out(sp, &buf, true) {
        return Err("stack frame write failed");
    }
    super::vm::add_stack_vma(asp); // SELFBUILD3: the main stack grows lazily to 8 MiB below the eager pages
    Ok(sp)
}
