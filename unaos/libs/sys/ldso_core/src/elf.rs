// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — shared-core
//!
//! SELFBUILD6 (B360): the byte-level ELF64 reading the loader needs — the header, the program headers, the dynamic
//! section's tags, the symbol record, the two hash functions. Everything is bounds-checked; nothing here allocates
//! except the program-header vector.

use alloc::vec::Vec;

pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;
pub const EM_X86_64: u16 = 62;

pub const PT_LOAD: u32 = 1;
pub const PT_DYNAMIC: u32 = 2;
pub const PT_INTERP: u32 = 3;
pub const PT_PHDR: u32 = 6;
pub const PT_TLS: u32 = 7;
pub const PT_GNU_EH_FRAME: u32 = 0x6474_E550;
pub const PT_GNU_STACK: u32 = 0x6474_E551;
pub const PT_GNU_RELRO: u32 = 0x6474_E552;

pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

pub const DT_NULL: u64 = 0;
pub const DT_NEEDED: u64 = 1;
pub const DT_PLTRELSZ: u64 = 2;
pub const DT_HASH: u64 = 4;
pub const DT_STRTAB: u64 = 5;
pub const DT_SYMTAB: u64 = 6;
pub const DT_RELA: u64 = 7;
pub const DT_RELASZ: u64 = 8;
pub const DT_RELAENT: u64 = 9;
pub const DT_STRSZ: u64 = 10;
pub const DT_SYMENT: u64 = 11;
pub const DT_INIT: u64 = 12;
pub const DT_FINI: u64 = 13;
pub const DT_SONAME: u64 = 14;
pub const DT_RPATH: u64 = 15;
pub const DT_REL: u64 = 17;
pub const DT_PLTREL: u64 = 20;
pub const DT_TEXTREL: u64 = 22;
pub const DT_JMPREL: u64 = 23;
pub const DT_INIT_ARRAY: u64 = 25;
pub const DT_FINI_ARRAY: u64 = 26;
pub const DT_INIT_ARRAYSZ: u64 = 27;
pub const DT_FINI_ARRAYSZ: u64 = 28;
pub const DT_RUNPATH: u64 = 29;
pub const DT_FLAGS: u64 = 30;
pub const DT_RELR: u64 = 36;
pub const DT_GNU_HASH: u64 = 0x6FFF_FEF5;
pub const DT_VERSYM: u64 = 0x6FFF_FFF0;
pub const DT_VERDEF: u64 = 0x6FFF_FFFC;
pub const DT_VERDEFNUM: u64 = 0x6FFF_FFFD;
pub const DT_VERNEED: u64 = 0x6FFF_FFFE;
pub const DT_VERNEEDNUM: u64 = 0x6FFF_FFFF;
pub const DF_TEXTREL: u64 = 4;

pub const SHN_UNDEF: u16 = 0;
pub const STB_LOCAL: u8 = 0;
pub const STB_GLOBAL: u8 = 1;
pub const STB_WEAK: u8 = 2;
pub const STB_GNU_UNIQUE: u8 = 10;
pub const STT_TLS: u8 = 6;
pub const STT_GNU_IFUNC: u8 = 10;

/// The relocation types the M1 probe found in rustc, its driver, libc.so and libgcc_s.so.1 — and no others.
pub const R_X86_64_NONE: u32 = 0;
pub const R_X86_64_64: u32 = 1;
pub const R_X86_64_GLOB_DAT: u32 = 6;
pub const R_X86_64_JUMP_SLOT: u32 = 7;
pub const R_X86_64_RELATIVE: u32 = 8;
pub const R_X86_64_DTPMOD64: u32 = 16;
pub const R_X86_64_DTPOFF64: u32 = 17;
pub const R_X86_64_TPOFF64: u32 = 18;

/// A relocation type's name, for the refusal line.
pub fn rel_name(t: u32) -> &'static str {
    match t {
        0 => "NONE",
        1 => "64",
        2 => "PC32",
        5 => "COPY",
        6 => "GLOB_DAT",
        7 => "JUMP_SLOT",
        8 => "RELATIVE",
        16 => "DTPMOD64",
        17 => "DTPOFF64",
        18 => "TPOFF64",
        36 => "TLSDESC",
        37 => "IRELATIVE",
        _ => "unknown",
    }
}

pub fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    b.get(o..o.checked_add(2)?).map(|s| u16::from_le_bytes([s[0], s[1]]))
}
pub fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o.checked_add(4)?).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}
pub fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    let s = b.get(o..o.checked_add(8)?)?;
    let mut a = [0u8; 8];
    a.copy_from_slice(s);
    Some(u64::from_le_bytes(a))
}

/// One program header.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Phdr {
    pub ty: u32,
    pub flags: u32,
    pub off: u64,
    pub vaddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub align: u64,
}

/// The ELF header fields the loader uses.
#[derive(Clone, Debug, Default)]
pub struct Ehdr {
    pub ty: u16,
    pub entry: u64,
    pub phoff: u64,
    pub phnum: u16,
    pub phdrs: Vec<Phdr>,
}

/// The byte count of the header + program-header table `head` says it needs (`Err` = not ELF64 x86-64).
pub fn head_len(head: &[u8]) -> Result<u64, &'static str> {
    if head.len() < 64 || head[0..4] != [0x7F, b'E', b'L', b'F'] {
        return Err("not an ELF image");
    }
    if head[4] != 2 || head[5] != 1 {
        return Err("not ELF64 little-endian");
    }
    let phoff = u64_at(head, 32).ok_or("bad e_phoff")?;
    let phnum = u16_at(head, 56).ok_or("bad e_phnum")? as u64;
    phoff.checked_add(phnum * 56).ok_or("phdr overflow")
}

/// Parse the header and program headers from `head` (which holds at least `head_len` bytes) of a file of `flen` bytes.
pub fn parse(head: &[u8], flen: u64) -> Result<Ehdr, &'static str> {
    let end = head_len(head)?;
    let ty = u16_at(head, 16).ok_or("bad e_type")?;
    if ty != ET_EXEC && ty != ET_DYN {
        return Err("not ET_EXEC or ET_DYN");
    }
    if u16_at(head, 18) != Some(EM_X86_64) {
        return Err("not EM_X86_64");
    }
    if u16_at(head, 54) != Some(56) {
        return Err("bad e_phentsize");
    }
    if end > head.len() as u64 {
        return Err("program-header table outside the head");
    }
    let entry = u64_at(head, 24).ok_or("bad e_entry")?;
    let phoff = u64_at(head, 32).ok_or("bad e_phoff")?;
    let phnum = u16_at(head, 56).ok_or("bad e_phnum")?;
    let mut phdrs = Vec::with_capacity(phnum as usize);
    for i in 0..phnum as usize {
        let o = phoff as usize + i * 56;
        let p = Phdr {
            ty: u32_at(head, o).ok_or("bad p_type")?,
            flags: u32_at(head, o + 4).ok_or("bad p_flags")?,
            off: u64_at(head, o + 8).ok_or("bad p_offset")?,
            vaddr: u64_at(head, o + 16).ok_or("bad p_vaddr")?,
            filesz: u64_at(head, o + 32).ok_or("bad p_filesz")?,
            memsz: u64_at(head, o + 40).ok_or("bad p_memsz")?,
            align: u64_at(head, o + 48).ok_or("bad p_align")?,
        };
        if p.ty == PT_LOAD {
            if p.filesz > p.memsz || p.off.checked_add(p.filesz).map_or(true, |e| e > flen) {
                return Err("segment file range outside the file");
            }
            if p.vaddr.checked_add(p.memsz).is_none() {
                return Err("segment vaddr overflow");
            }
            if p.flags & PF_W != 0 && p.flags & PF_X != 0 {
                return Err("W+X segment refused (W^X)");
            }
            if (p.vaddr ^ p.off) & 0xFFF != 0 {
                return Err("segment offset and vaddr disagree modulo the page");
            }
        }
        phdrs.push(p);
    }
    if !phdrs.iter().any(|p| p.ty == PT_LOAD) {
        return Err("no PT_LOAD segments");
    }
    Ok(Ehdr { ty, entry, phoff, phnum, phdrs })
}

/// The file offset holding `vaddr .. vaddr+len` (inside one PT_LOAD's file bytes).
pub fn file_off(phdrs: &[Phdr], vaddr: u64, len: u64) -> Option<u64> {
    phdrs.iter().filter(|p| p.ty == PT_LOAD).find_map(|p| {
        let end = vaddr.checked_add(len)?;
        if vaddr >= p.vaddr && end <= p.vaddr + p.filesz { Some(p.off + (vaddr - p.vaddr)) } else { None }
    })
}

/// One dynamic symbol (`Elf64_Sym`, 24 bytes).
#[derive(Clone, Copy, Debug, Default)]
pub struct Sym {
    pub name: u32,
    pub info: u8,
    pub shndx: u16,
    pub value: u64,
    pub size: u64,
}

impl Sym {
    pub fn at(tab: &[u8], i: usize) -> Option<Sym> {
        let o = i.checked_mul(24)?;
        Some(Sym {
            name: u32_at(tab, o)?,
            info: *tab.get(o + 4)?,
            shndx: u16_at(tab, o + 6)?,
            value: u64_at(tab, o + 8)?,
            size: u64_at(tab, o + 16)?,
        })
    }
    pub fn bind(&self) -> u8 {
        self.info >> 4
    }
    pub fn kind(&self) -> u8 {
        self.info & 0xF
    }
    pub fn defined(&self) -> bool {
        self.shndx != SHN_UNDEF && matches!(self.bind(), STB_GLOBAL | STB_WEAK | STB_GNU_UNIQUE)
    }
}

/// The NUL-terminated string at `off` in `tab`.
pub fn cstr(tab: &[u8], off: usize) -> &[u8] {
    let s = tab.get(off..).unwrap_or(&[]);
    let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    &s[..n]
}

/// The GNU hash (`DT_GNU_HASH`): djb2, 32 bits.
pub fn gnu_hash(name: &[u8]) -> u32 {
    let mut h: u32 = 5381;
    for &c in name {
        h = h.wrapping_mul(33).wrapping_add(c as u32);
    }
    h
}

/// The System V ELF hash (`DT_HASH`, and the hash a VERNEED/VERDEF entry carries).
pub fn elf_hash(name: &[u8]) -> u32 {
    let mut h: u32 = 0;
    for &c in name {
        h = (h << 4).wrapping_add(c as u32);
        let g = h & 0xF000_0000;
        if g != 0 {
            h ^= g >> 24;
        }
        h &= !g;
    }
    h
}
