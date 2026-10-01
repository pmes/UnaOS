// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! LINUXABI M1 — static Linux x86_64 ELF: parse, map at the FIXED vaddrs, build the System V stack.
//!
//! Accepts `ET_EXEC` / `EM_X86_64` with no `PT_INTERP` only (static, non-PIE — what musl-static busybox and
//! a static toolchain are). Everything read from the image is bounds-checked; W+X segments are refused.

use super::{AddrSpace, PAGE, STACK_PAGES, STACK_TOP};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

const PT_LOAD: u32 = 1;
const PT_INTERP: u32 = 3;
const MAX_SEGS: usize = 16;
/// The image must fit below the kernel heap (which starts at >= 16 MiB): the process's view of low
/// memory REPLACES the kernel's identity mapping under its CR3, so these pages must be free RAM.
const IMAGE_LIMIT: u64 = 0x0100_0000;
const IMAGE_FLOOR: u64 = 0x1_0000;

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
    if b.len() < 64 || b[0..4] != [0x7F, b'E', b'L', b'F'] {
        return Err("not an ELF image");
    }
    if b[4] != 2 || b[5] != 1 {
        return Err("not ELF64 little-endian");
    }
    if u16_at(b, 16) != Some(2) {
        return Err("not ET_EXEC (static-PIE / shared objects are not supported)");
    }
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
        if ty == PT_INTERP {
            return Err("dynamic executable (PT_INTERP) — only static binaries are supported");
        }
        if ty != PT_LOAD {
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
        if filesz > memsz || off.checked_add(filesz).map_or(true, |e| e > b.len() as u64) {
            return Err("segment file range outside the image");
        }
        let end = vaddr.checked_add(memsz).ok_or("segment vaddr overflow")?;
        if vaddr < IMAGE_FLOOR || end > IMAGE_LIMIT {
            return Err("segment outside the loadable window (0x10000..16 MiB)");
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
    if total > (64 << 20) {
        return Err("image too large");
    }
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
    Ok(Plan { entry, segs, phdr_va, phnum, phent })
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
    if !super::memory::region_is_usable(lo, hi - lo) {
        return Err("load range is not free RAM on this machine (would overlay the kernel)");
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
    let aux: [(u64, u64); 17] = [
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
        (16, 0),
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
    Ok(sp)
}
