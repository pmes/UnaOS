// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — shared-core
//!
//! SELFBUILD6 (rmbp-ledger B360): the ONE dynamic loader both rings link — the kernel's Linux ABI shim
//! (`arch/x86_64/linuxabi/ldso.rs`, over the process's VMAs) and the host runner (`src/bin/ldrun.rs`, over `mmap` on Linux).
//! It is not musl's `ld.so` (C; R79). The program's libc IS musl's static `libc.a`, relinked `-shared` as `libc.so`; this
//! core does the part `ld.so` would have done before entering it:
//!
//! * map the executable and every `DT_NEEDED` object (breadth-first, `DT_RPATH`/`DT_RUNPATH` with `$ORIGIN`, then the
//!   fulfiller's search list) at first-fit bases — ASLR is OFF; an `ET_EXEC` goes at its own addresses;
//! * resolve symbols by GNU hash (SysV hash as the fallback) with symbol versioning (`DT_VERSYM` / `DT_VERNEED` /
//!   `DT_VERDEF`) in load order, the loader's own table first;
//! * apply exactly the seven relocation types the M1 probe found (`RELATIVE GLOB_DAT JUMP_SLOT 64 TPOFF64 DTPMOD64
//!   DTPOFF64`) and refuse every other one by name;
//! * lay out static TLS (variant II, [`tls`]) for every object in ONE synthetic `PT_TLS` that musl's static `__init_tls`
//!   reads from `AT_PHDR`;
//! * order constructors (dependencies first) and destructors (the reverse) for the ring-3 trampoline's start hook;
//! * keep the `dl_iterate_phdr` table and the `dlopen` / `dlsym` / `dlclose` / `dladdr` table.
//!
//! The ring-3 half is one page of position-independent machine code ([`tramp_bytes`], assembled from `src/tramp.S`)
//! plus one data page whose slots are the `DATA_*` offsets below.
//!
//! Stays owed (said in the design doc): RELRO is not re-protected after relocation; `dlclose` keeps the object mapped;
//! a `dlopen`ed object's TLS initial image reaches the calling thread and later threads, not threads already running;
//! no IFUNC / IRELATIVE / TLSDESC / COPY / RELR / TEXTREL (none in the payload).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod elf;
pub mod tls;
pub mod tramp_bytes;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use elf::*;

pub const PAGE: u64 = 4096;
pub const PROT_R: u32 = 1;
pub const PROT_W: u32 = 2;
pub const PROT_X: u32 = 4;

// ---- the trampoline's data page (offsets; `src/tramp.S` names them) ----
pub const DATA_REAL_MAIN: u64 = 0x00;
pub const DATA_LIBC_START: u64 = 0x08;
pub const DATA_INIT_LIST: u64 = 0x10;
pub const DATA_FINI_LIST: u64 = 0x18;
pub const DATA_ATEXIT: u64 = 0x20;
pub const DATA_G_OPEN: u64 = 0x28;
pub const DATA_G_SYM: u64 = 0x30;
pub const DATA_G_CLOSE: u64 = 0x38;
pub const DATA_ERRSTR: u64 = 0x40;
pub const DATA_OBJ_TAB: u64 = 0x48;
pub const DATA_OBJ_N: u64 = 0x50;
pub const DATA_PENDING: u64 = 0x58;
pub const DATA_G_ADDR: u64 = 0x60;

/// `dlopen` flags the table honours.
pub const RTLD_GLOBAL: u64 = 0x100;
pub const RTLD_NOLOAD: u64 = 0x4;
/// `dlsym` pseudo-handles.
pub const RTLD_DEFAULT: u64 = 0;
pub const RTLD_NEXT: u64 = u64::MAX;

/// The kernel's private gate syscalls (the trampoline's `t_gate_*`).
pub const SYS_DLOPEN: u64 = 7504;
pub const SYS_DLSYM: u64 = 7505;
pub const SYS_DLCLOSE: u64 = 7506;
pub const SYS_DLADDR: u64 = 7507;

/// The names the loader itself defines, first in every lookup, and their trampoline entries.
pub const LOADER_SYMS: [(&[u8], u64); 8] = [
    (b"__libc_start_main", tramp_bytes::T_START_MAIN),
    (b"dlopen", tramp_bytes::T_DLOPEN),
    (b"dlsym", tramp_bytes::T_DLSYM),
    (b"dlclose", tramp_bytes::T_DLCLOSE),
    (b"dladdr", tramp_bytes::T_DLADDR),
    (b"dlerror", tramp_bytes::T_DLERROR),
    (b"__tls_get_addr", tramp_bytes::T_TLS_GET_ADDR),
    (b"dl_iterate_phdr", tramp_bytes::T_DL_ITERATE_PHDR),
];

/// The address space the loader works in. The kernel fulfils it over a process's VMAs; the host runner over `mmap`.
/// Metadata (headers, dynamic tables, relocations) is READ FROM THE FILE, never from the mapping, so a lazy mapping is
/// only faulted in where a relocation writes.
pub trait Space {
    type File: Clone;
    /// Open `path` for reading: the handle and the file's size.
    fn open(&mut self, path: &[u8]) -> Option<(Self::File, u64)>;
    /// Read exactly `buf.len()` bytes at `off`.
    fn read_file(&mut self, f: &Self::File, off: u64, buf: &mut [u8]) -> bool;
    /// Reserve `len` bytes (page multiple): first fit, or exactly at `fixed`. The range is inaccessible until mapped.
    fn reserve(&mut self, len: u64, fixed: Option<u64>) -> Result<u64, String>;
    /// Map `len` bytes of `f` at file offset `off` (both page-aligned) privately at `va`, inside a reservation.
    fn map_file(&mut self, va: u64, len: u64, f: &Self::File, off: u64, prot: u32) -> bool;
    /// Map `len` zero bytes at `va`, inside a reservation (`prot` 0 = a hole: PROT_NONE).
    fn map_anon(&mut self, va: u64, len: u64, prot: u32) -> bool;
    /// Map one page of code (read + execute) at `va` holding `bytes`.
    fn map_code(&mut self, va: u64, bytes: &[u8]) -> bool;
    /// Store `data` at `va` as the loader (protection ignored: relocations land in RELRO before the program runs).
    fn write(&mut self, va: u64, data: &[u8]) -> bool;
    /// Load `out.len()` bytes from `va`.
    fn read(&mut self, va: u64, out: &mut [u8]) -> bool;
}

/// GNU hash table (`DT_GNU_HASH`), resident.
#[derive(Clone, Debug, Default)]
struct Gnu {
    nbuckets: u32,
    symoffset: u32,
    shift: u32,
    bloom: Vec<u64>,
    buckets: Vec<u32>,
    chain: Vec<u32>,
}

/// SysV hash table (`DT_HASH`), resident.
#[derive(Clone, Debug, Default)]
struct Sysv {
    buckets: Vec<u32>,
    chain: Vec<u32>,
}

/// One loaded object.
#[derive(Clone)]
pub struct Obj<F> {
    /// The path it was opened by.
    pub path: Vec<u8>,
    pub soname: Vec<u8>,
    pub file: F,
    pub ty: u16,
    /// Load bias (0 for `ET_EXEC`).
    pub base: u64,
    /// The mapped span `[lo, hi)`.
    pub lo: u64,
    pub hi: u64,
    pub phdrs: Vec<Phdr>,
    /// Where its program headers are in memory (the `dl_iterate_phdr` record's `dlpi_phdr`).
    pub phdr_va: u64,
    pub entry: u64,
    dynsym: Vec<u8>,
    dynstr: Vec<u8>,
    versym: Vec<u16>,
    gnu: Option<Gnu>,
    sysv: Option<Sysv>,
    /// (vd_ndx, name) of each version definition.
    verdef: Vec<(u16, Vec<u8>)>,
    /// (vna_other, name) of each version requirement.
    verneed: Vec<(u16, Vec<u8>)>,
    strtab_va: u64,
    rela: (u64, u64),
    jmprel: (u64, u64),
    init: u64,
    init_array: (u64, u64),
    fini: u64,
    fini_array: (u64, u64),
    needed: Vec<Vec<u8>>,
    rpath: Vec<Vec<u8>>,
    runpath: Vec<Vec<u8>>,
    /// Indices of its `DT_NEEDED` objects.
    pub deps: Vec<usize>,
    pub tls: Option<tls::Mod>,
    tls_off: u64,
    /// Distance of its TLS block below the thread pointer (TP offset = `-tls_dist`).
    pub tls_dist: u64,
    pub tls_id: u64,
    /// Relocations applied in it.
    pub relocs: u64,
    pub relocated: bool,
    /// Its constructors already ran (or were handed to the trampoline).
    inited: bool,
    /// libc.so runs its own constructors from `__libc_start_main`.
    is_libc: bool,
    /// The root of the `dlopen` group it came in with (`usize::MAX` = the initial load).
    group: usize,
    refs: u32,
    /// Its path as a C string in the program's memory.
    name_va: u64,
}

/// What the program is entered with.
#[derive(Clone, Copy, Debug, Default)]
pub struct Start {
    /// `AT_ENTRY` / the jump target.
    pub entry: u64,
    /// `AT_PHDR` / `AT_PHNUM` / `AT_PHENT`: the SYNTHETIC table musl's static start-up reads (PT_PHDR, PT_TLS, PT_GNU_STACK).
    pub phdr: u64,
    pub phnum: u64,
    pub phent: u64,
    /// `AT_BASE`: the trampoline page (there is no separate interpreter image).
    pub tramp: u64,
}

/// The numbers the wire and the doc carry.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub objects: u64,
    pub pages_mapped: u64,
    pub relocs: u64,
    pub lookups: u64,
    pub file_bytes_read: u64,
    pub dlopens: u64,
}

/// The loader: the objects, the global scope, the trampoline and its arena.
#[derive(Clone)]
pub struct Loader<F> {
    pub objs: Vec<Obj<F>>,
    /// Lookup order (indices; the loader's own table precedes it).
    pub global: Vec<usize>,
    /// Directories searched after `DT_RPATH`/`DT_RUNPATH`.
    pub search: Vec<Vec<u8>>,
    pub tramp: u64,
    pub data: u64,
    arena: (u64, u64),
    tls_template: u64,
    tls_size: u64,
    tls_used: u64,
    tls_next_id: u64,
    pub stats: Stats,
}

type R<T> = Result<T, String>;

fn s(b: &[u8]) -> &str {
    core::str::from_utf8(b).unwrap_or("?")
}

fn page_down(v: u64) -> u64 {
    v & !(PAGE - 1)
}
fn page_up(v: u64) -> u64 {
    (v + PAGE - 1) & !(PAGE - 1)
}

fn basename(p: &[u8]) -> &[u8] {
    match p.iter().rposition(|&c| c == b'/') {
        Some(i) => &p[i + 1..],
        None => p,
    }
}

fn dirname(p: &[u8]) -> &[u8] {
    match p.iter().rposition(|&c| c == b'/') {
        Some(0) => b"/",
        Some(i) => &p[..i],
        None => b".",
    }
}

impl<F: Clone> Loader<F> {
    pub fn new(search: Vec<Vec<u8>>) -> Self {
        Loader {
            objs: Vec::new(),
            global: Vec::new(),
            search,
            tramp: 0,
            data: 0,
            arena: (0, 0),
            tls_template: 0,
            tls_size: 0,
            tls_used: 0,
            tls_next_id: 1,
            stats: Stats::default(),
        }
    }

    // ---------------------------------------------------------------------------------------------
    // reading the file
    // ---------------------------------------------------------------------------------------------

    fn fread<S: Space<File = F>>(&mut self, sp: &mut S, f: &F, off: u64, len: u64) -> R<Vec<u8>> {
        if len > (256 << 20) {
            return Err(format!("table of {} bytes refused", len));
        }
        let mut v = vec![0u8; len as usize];
        if !sp.read_file(f, off, &mut v) {
            return Err(format!("read of {} bytes at {} failed", len, off));
        }
        self.stats.file_bytes_read += len;
        Ok(v)
    }

    /// `len` bytes of the object at link-time address `vaddr`, from the file.
    fn vread<S: Space<File = F>>(&mut self, sp: &mut S, f: &F, phdrs: &[Phdr], vaddr: u64, len: u64) -> R<Vec<u8>> {
        let off = file_off(phdrs, vaddr, len).ok_or_else(|| format!("vaddr {:#x}+{} is not in the file", vaddr, len))?;
        self.fread(sp, f, off, len)
    }

    // ---------------------------------------------------------------------------------------------
    // mapping one object
    // ---------------------------------------------------------------------------------------------

    /// Map the object at `path` and read its dynamic tables. Returns its index.
    pub fn map_object<S: Space<File = F>>(&mut self, sp: &mut S, path: &[u8], file: F, size: u64) -> R<usize> {
        let mut head = self.fread(sp, &file, 0, size.min(PAGE))?;
        let need = head_len(&head).map_err(|e| format!("{}: {}", s(path), e))?;
        if need > head.len() as u64 {
            if need > size || need > 65536 {
                return Err(format!("{}: program-header table out of range", s(path)));
            }
            head = self.fread(sp, &file, 0, need)?;
        }
        let eh = parse(&head, size).map_err(|e| format!("{}: {}", s(path), e))?;
        let loads: Vec<Phdr> = eh.phdrs.iter().copied().filter(|p| p.ty == PT_LOAD).collect();
        let lo = loads.iter().map(|p| page_down(p.vaddr)).min().unwrap_or(0);
        let hi = loads.iter().map(|p| page_up(p.vaddr + p.memsz)).max().unwrap_or(0);
        // Segments must not share a page (lld and the payload never do; a shared page would need an eager merge).
        let mut spans: Vec<(u64, u64)> = loads.iter().map(|p| (page_down(p.vaddr), page_up(p.vaddr + p.memsz))).collect();
        spans.sort();
        for w in spans.windows(2) {
            if w[1].0 < w[0].1 {
                return Err(format!("{}: two PT_LOAD segments share a page", s(path)));
            }
        }
        let fixed = if eh.ty == ET_EXEC { Some(lo) } else { None };
        let at = sp.reserve(hi - lo, fixed).map_err(|e| format!("{}: {}", s(path), e))?;
        let base = if eh.ty == ET_EXEC { 0 } else { at - lo };
        // Segments, then the holes between them (PROT_NONE, owned by the object).
        let mut cur = lo;
        for &(a, b) in &spans {
            if a > cur && !sp.map_anon(base + cur, a - cur, 0) {
                return Err(format!("{}: hole mapping failed", s(path)));
            }
            cur = cur.max(b);
        }
        for p in &loads {
            let prot = (if p.flags & PF_R != 0 { PROT_R } else { 0 })
                | (if p.flags & PF_W != 0 { PROT_W } else { 0 })
                | (if p.flags & PF_X != 0 { PROT_X } else { 0 });
            let fstart = page_down(p.vaddr);
            let fend = p.vaddr + p.filesz;
            let mend = page_up(p.vaddr + p.memsz);
            let file_pages_end = if p.filesz > 0 { page_up(fend) } else { fstart };
            if file_pages_end > fstart {
                if !sp.map_file(base + fstart, file_pages_end - fstart, &file, page_down(p.off), prot) {
                    return Err(format!("{}: segment mapping failed at {:#x}", s(path), base + fstart));
                }
                // The page holding the file bytes' end: what follows them in the file is other sections, never .bss.
                if p.memsz > p.filesz && fend & (PAGE - 1) != 0 {
                    let z = vec![0u8; (file_pages_end - fend) as usize];
                    if !sp.write(base + fend, &z) {
                        return Err(format!("{}: .bss head zeroing failed", s(path)));
                    }
                }
            }
            if mend > file_pages_end && !sp.map_anon(base + file_pages_end, mend - file_pages_end, prot) {
                return Err(format!("{}: .bss mapping failed", s(path)));
            }
        }
        self.stats.pages_mapped += (hi - lo) / PAGE;
        self.stats.objects += 1;
        // Program headers in memory.
        let phdr_va = match eh.phdrs.iter().find(|p| p.ty == PT_PHDR) {
            Some(p) => base + p.vaddr,
            None => loads
                .iter()
                .find(|p| eh.phoff >= p.off && eh.phoff + eh.phnum as u64 * 56 <= p.off + p.filesz)
                .map(|p| base + p.vaddr + (eh.phoff - p.off))
                .unwrap_or(0),
        };
        let mut o = Obj {
            path: path.to_vec(),
            soname: Vec::new(),
            file: file.clone(),
            ty: eh.ty,
            base,
            lo: base + lo,
            hi: base + hi,
            phdrs: eh.phdrs.clone(),
            phdr_va,
            entry: base + eh.entry,
            dynsym: Vec::new(),
            dynstr: Vec::new(),
            versym: Vec::new(),
            gnu: None,
            sysv: None,
            verdef: Vec::new(),
            verneed: Vec::new(),
            strtab_va: 0,
            rela: (0, 0),
            jmprel: (0, 0),
            init: 0,
            init_array: (0, 0),
            fini: 0,
            fini_array: (0, 0),
            needed: Vec::new(),
            rpath: Vec::new(),
            runpath: Vec::new(),
            deps: Vec::new(),
            tls: None,
            tls_off: 0,
            tls_dist: 0,
            tls_id: 0,
            relocs: 0,
            relocated: false,
            inited: false,
            is_libc: false,
            group: usize::MAX,
            refs: 1,
            name_va: 0,
        };
        if let Some(t) = eh.phdrs.iter().find(|p| p.ty == PT_TLS) {
            o.tls = Some(tls::Mod { vaddr: t.vaddr, filesz: t.filesz, memsz: t.memsz, align: t.align });
            o.tls_off = t.off;
        }
        if let Some(d) = eh.phdrs.iter().find(|p| p.ty == PT_DYNAMIC) {
            self.read_dynamic(sp, &mut o, d.off, d.filesz)?;
        }
        self.objs.push(o);
        Ok(self.objs.len() - 1)
    }

    fn read_dynamic<S: Space<File = F>>(&mut self, sp: &mut S, o: &mut Obj<F>, off: u64, len: u64) -> R<()> {
        let d = self.fread(sp, &o.file.clone(), off, len)?;
        let mut tags: Vec<(u64, u64)> = Vec::new();
        let mut i = 0;
        while i + 16 <= d.len() {
            let t = u64_at(&d, i).unwrap_or(0);
            let v = u64_at(&d, i + 8).unwrap_or(0);
            if t == DT_NULL {
                break;
            }
            tags.push((t, v));
            i += 16;
        }
        let get = |t: u64| tags.iter().find(|x| x.0 == t).map(|x| x.1);
        let p = o.path.clone();
        if get(DT_TEXTREL).is_some() || get(DT_FLAGS).is_some_and(|f| f & DF_TEXTREL != 0) {
            return Err(format!("{}: text relocations refused", s(&p)));
        }
        if get(DT_REL).is_some() || get(DT_RELR).is_some() {
            return Err(format!("{}: DT_REL / DT_RELR refused (x86-64 RELA only)", s(&p)));
        }
        if get(DT_RELAENT).is_some_and(|e| e != 24) || get(DT_SYMENT).is_some_and(|e| e != 24) {
            return Err(format!("{}: odd RELAENT/SYMENT", s(&p)));
        }
        let phdrs = o.phdrs.clone();
        let f = o.file.clone();
        let strtab = get(DT_STRTAB).ok_or_else(|| format!("{}: no DT_STRTAB", s(&p)))?;
        let strsz = get(DT_STRSZ).unwrap_or(0);
        o.dynstr = self.vread(sp, &f, &phdrs, strtab, strsz)?;
        o.strtab_va = o.base + strtab;
        let symtab = get(DT_SYMTAB).ok_or_else(|| format!("{}: no DT_SYMTAB", s(&p)))?;
        // The symbol count: from the GNU hash chains, else the SysV hash's nchain.
        let mut nsyms = 0u64;
        if let Some(gh) = get(DT_GNU_HASH) {
            let h = self.vread(sp, &f, &phdrs, gh, 16)?;
            let (nb, so, bs, sh) = (u32_at(&h, 0).unwrap(), u32_at(&h, 4).unwrap(), u32_at(&h, 8).unwrap(), u32_at(&h, 12).unwrap());
            if nb == 0 || bs == 0 || !bs.is_power_of_two() {
                return Err(format!("{}: bad DT_GNU_HASH", s(&p)));
            }
            let body = self.vread(sp, &f, &phdrs, gh + 16, bs as u64 * 8 + nb as u64 * 4)?;
            let bloom: Vec<u64> = (0..bs as usize).map(|k| u64_at(&body, k * 8).unwrap()).collect();
            let buckets: Vec<u32> = (0..nb as usize).map(|k| u32_at(&body, bs as usize * 8 + k * 4).unwrap()).collect();
            let chain_va = gh + 16 + bs as u64 * 8 + nb as u64 * 4;
            let maxb = buckets.iter().copied().max().unwrap_or(0);
            let mut chain = Vec::new();
            if maxb >= so {
                // Chains run from symoffset to the end of the last bucket's chain.
                let mut n = (maxb - so) as u64;
                let mut buf = self.vread(sp, &f, &phdrs, chain_va, (n + 1) * 4)?;
                loop {
                    let v = u32_at(&buf, n as usize * 4).unwrap();
                    if v & 1 != 0 {
                        break;
                    }
                    n += 1;
                    if (n as usize + 1) * 4 > buf.len() {
                        // Read on in steps, never past the segment's file bytes.
                        let at = chain_va + buf.len() as u64;
                        let left = phdrs
                            .iter()
                            .filter(|q| q.ty == PT_LOAD && at >= q.vaddr && at < q.vaddr + q.filesz)
                            .map(|q| q.vaddr + q.filesz - at)
                            .next()
                            .unwrap_or(0)
                            & !3;
                        if left == 0 || buf.len() > (1 << 24) {
                            return Err(format!("{}: GNU hash chain runs off", s(&p)));
                        }
                        let more = self.vread(sp, &f, &phdrs, at, left.min(4096))?;
                        buf.extend_from_slice(&more);
                    }
                }
                chain = (0..=n as usize).map(|k| u32_at(&buf, k * 4).unwrap()).collect();
                nsyms = so as u64 + n + 1;
            } else {
                nsyms = so as u64;
            }
            o.gnu = Some(Gnu { nbuckets: nb, symoffset: so, shift: sh, bloom, buckets, chain });
        }
        if let Some(hh) = get(DT_HASH) {
            let h = self.vread(sp, &f, &phdrs, hh, 8)?;
            let (nb, nc) = (u32_at(&h, 0).unwrap() as u64, u32_at(&h, 4).unwrap() as u64);
            if o.gnu.is_none() {
                let body = self.vread(sp, &f, &phdrs, hh + 8, (nb + nc) * 4)?;
                let buckets = (0..nb as usize).map(|k| u32_at(&body, k * 4).unwrap()).collect();
                let chain = (0..nc as usize).map(|k| u32_at(&body, (nb as usize + k) * 4).unwrap()).collect();
                o.sysv = Some(Sysv { buckets, chain });
            }
            nsyms = nsyms.max(nc);
        }
        if nsyms == 0 && o.gnu.is_none() && o.sysv.is_none() {
            // No hash table: the symbols still serve this object's own relocations (count bounded by the strtab start).
            if strtab > symtab {
                nsyms = (strtab - symtab) / 24;
            }
        }
        o.dynsym = self.vread(sp, &f, &phdrs, symtab, nsyms * 24)?;
        if let Some(vs) = get(DT_VERSYM) {
            let b = self.vread(sp, &f, &phdrs, vs, nsyms * 2)?;
            o.versym = (0..nsyms as usize).map(|k| u16_at(&b, k * 2).unwrap()).collect();
        }
        if let (Some(vd), Some(n)) = (get(DT_VERDEF), get(DT_VERDEFNUM)) {
            let mut at = vd;
            for _ in 0..n.min(4096) {
                let e = self.vread(sp, &f, &phdrs, at, 20)?;
                let ndx = u16_at(&e, 4).unwrap();
                let aux = u32_at(&e, 12).unwrap() as u64;
                let next = u32_at(&e, 16).unwrap() as u64;
                let a = self.vread(sp, &f, &phdrs, at + aux, 8)?;
                let name = cstr(&o.dynstr, u32_at(&a, 0).unwrap() as usize).to_vec();
                o.verdef.push((ndx, name));
                if next == 0 {
                    break;
                }
                at += next;
            }
        }
        if let (Some(vn), Some(n)) = (get(DT_VERNEED), get(DT_VERNEEDNUM)) {
            let mut at = vn;
            for _ in 0..n.min(4096) {
                let e = self.vread(sp, &f, &phdrs, at, 16)?;
                let cnt = u16_at(&e, 2).unwrap();
                let aux = u32_at(&e, 8).unwrap() as u64;
                let next = u32_at(&e, 12).unwrap() as u64;
                let mut a_at = at + aux;
                for _ in 0..cnt {
                    let a = self.vread(sp, &f, &phdrs, a_at, 16)?;
                    let other = u16_at(&a, 6).unwrap();
                    let name = cstr(&o.dynstr, u32_at(&a, 8).unwrap() as usize).to_vec();
                    o.verneed.push((other, name));
                    let an = u32_at(&a, 12).unwrap() as u64;
                    if an == 0 {
                        break;
                    }
                    a_at += an;
                }
                if next == 0 {
                    break;
                }
                at += next;
            }
        }
        let str_of = |v: u64, o: &Obj<F>| cstr(&o.dynstr, v as usize).to_vec();
        for &(t, v) in &tags {
            match t {
                DT_NEEDED => {
                    let n = str_of(v, o);
                    o.needed.push(n);
                }
                DT_SONAME => o.soname = str_of(v, o),
                DT_RPATH => o.rpath = str_of(v, o).split(|&c| c == b':').map(|x| x.to_vec()).collect(),
                DT_RUNPATH => o.runpath = str_of(v, o).split(|&c| c == b':').map(|x| x.to_vec()).collect(),
                _ => {}
            }
        }
        o.rela = (get(DT_RELA).unwrap_or(0), get(DT_RELASZ).unwrap_or(0));
        if get(DT_JMPREL).is_some() {
            if get(DT_PLTREL).is_some_and(|k| k != DT_RELA) {
                return Err(format!("{}: DT_PLTREL is not RELA", s(&p)));
            }
            o.jmprel = (get(DT_JMPREL).unwrap_or(0), get(DT_PLTRELSZ).unwrap_or(0));
        }
        o.init = get(DT_INIT).map_or(0, |v| o.base + v);
        o.fini = get(DT_FINI).map_or(0, |v| o.base + v);
        o.init_array = (get(DT_INIT_ARRAY).map_or(0, |v| o.base + v), get(DT_INIT_ARRAYSZ).unwrap_or(0) / 8);
        o.fini_array = (get(DT_FINI_ARRAY).map_or(0, |v| o.base + v), get(DT_FINI_ARRAYSZ).unwrap_or(0) / 8);
        Ok(())
    }

    // ---------------------------------------------------------------------------------------------
    // finding and loading dependencies
    // ---------------------------------------------------------------------------------------------

    fn loaded(&self, name: &[u8]) -> Option<usize> {
        self.objs.iter().position(|o| o.soname == name || o.path == name || (!name.contains(&b'/') && basename(&o.path) == name))
    }

    fn find_lib<S: Space<File = F>>(&self, sp: &mut S, name: &[u8], needer: usize) -> Option<(Vec<u8>, F, u64)> {
        if name.contains(&b'/') {
            return sp.open(name).map(|(f, n)| (name.to_vec(), f, n));
        }
        let mut dirs: Vec<Vec<u8>> = Vec::new();
        let expand = |d: &[u8], who: &Obj<F>| -> Vec<u8> {
            let mut out = Vec::new();
            let origin = dirname(&who.path);
            let mut i = 0;
            while i < d.len() {
                if d[i..].starts_with(b"$ORIGIN") {
                    out.extend_from_slice(origin);
                    i += 7;
                } else if d[i..].starts_with(b"${ORIGIN}") {
                    out.extend_from_slice(origin);
                    i += 9;
                } else {
                    out.push(d[i]);
                    i += 1;
                }
            }
            out
        };
        let n = &self.objs[needer];
        if n.runpath.is_empty() {
            for d in &n.rpath {
                dirs.push(expand(d, n));
            }
            if needer != 0 && !self.objs.is_empty() && self.objs[0].runpath.is_empty() {
                for d in &self.objs[0].rpath {
                    dirs.push(expand(d, &self.objs[0]));
                }
            }
        } else {
            for d in &n.runpath {
                dirs.push(expand(d, n));
            }
        }
        dirs.extend(self.search.iter().cloned());
        for d in dirs {
            let mut p = d.clone();
            if !p.ends_with(b"/") {
                p.push(b'/');
            }
            p.extend_from_slice(name);
            if let Some((f, size)) = sp.open(&p) {
                return Some((p, f, size));
            }
        }
        None
    }

    /// Load every `DT_NEEDED` of `from..` breadth-first (new objects join `group`).
    fn load_deps<S: Space<File = F>>(&mut self, sp: &mut S, from: usize, group: usize) -> R<()> {
        let mut q = from;
        while q < self.objs.len() {
            let needed = self.objs[q].needed.clone();
            for n in needed {
                let idx = match self.loaded(&n) {
                    Some(i) => {
                        if self.objs[i].group != usize::MAX && group != self.objs[i].group {
                            self.objs[i].refs += 1;
                        }
                        i
                    }
                    None => {
                        let (p, f, size) = self.find_lib(sp, &n, q).ok_or_else(|| {
                            format!("{}: needed {} not found (search: {})", s(&self.objs[q].path), s(&n), self.search_str())
                        })?;
                        let i = self.map_object(sp, &p, f, size)?;
                        self.objs[i].group = group;
                        i
                    }
                };
                if !self.objs[q].deps.contains(&idx) {
                    self.objs[q].deps.push(idx);
                }
            }
            q += 1;
        }
        Ok(())
    }

    fn search_str(&self) -> String {
        let mut o = String::new();
        for (i, d) in self.search.iter().enumerate() {
            if i > 0 {
                o.push(':');
            }
            o.push_str(s(d));
        }
        o
    }

    // ---------------------------------------------------------------------------------------------
    // symbols
    // ---------------------------------------------------------------------------------------------

    /// The version a reference at `idx` in `o` requires (`None` = unversioned).
    fn want_version(o: &Obj<F>, idx: usize) -> Option<&[u8]> {
        let v = *o.versym.get(idx)? & 0x7FFF;
        if v <= 1 {
            return None;
        }
        o.verneed.iter().find(|x| x.0 == v).map(|x| x.1.as_slice())
    }

    fn version_ok(o: &Obj<F>, i: usize, want: Option<&[u8]>) -> bool {
        let Some(&vs) = o.versym.get(i) else { return true };
        let idx = vs & 0x7FFF;
        match want {
            None => vs & 0x8000 == 0,
            Some(w) => {
                if idx <= 1 {
                    return true;
                }
                match o.verdef.iter().find(|x| x.0 == idx) {
                    Some((_, n)) => n.as_slice() == w,
                    None => true,
                }
            }
        }
    }

    /// The defined symbol `name` in object `oi` (index into its dynsym).
    fn find_in(&self, oi: usize, name: &[u8], gh: u32, eh: u32, want: Option<&[u8]>) -> Option<usize> {
        let o = &self.objs[oi];
        let check = |i: usize| -> bool {
            let Some(sym) = Sym::at(&o.dynsym, i) else { return false };
            sym.defined() && sym.kind() != STT_GNU_IFUNC && cstr(&o.dynstr, sym.name as usize) == name && Self::version_ok(o, i, want)
        };
        if let Some(g) = &o.gnu {
            let bits = 64u32;
            let word = g.bloom[((gh / bits) as usize) & (g.bloom.len() - 1)];
            let mask = (1u64 << (gh % bits)) | (1u64 << ((gh >> g.shift) % bits));
            if word & mask != mask {
                return None;
            }
            let mut i = g.buckets[(gh % g.nbuckets) as usize];
            if i < g.symoffset {
                return None;
            }
            loop {
                let ch = *g.chain.get((i - g.symoffset) as usize)?;
                if (ch | 1) == (gh | 1) && check(i as usize) {
                    return Some(i as usize);
                }
                if ch & 1 != 0 {
                    return None;
                }
                i += 1;
            }
        }
        if let Some(h) = &o.sysv {
            if h.buckets.is_empty() {
                return None;
            }
            let mut i = h.buckets[(eh as usize) % h.buckets.len()];
            let mut guard = 0;
            while i != 0 && guard < h.chain.len() + 1 {
                if check(i as usize) {
                    return Some(i as usize);
                }
                i = *h.chain.get(i as usize)?;
                guard += 1;
            }
            return None;
        }
        (0..o.dynsym.len() / 24).find(|&i| check(i))
    }

    /// The scope a reference from object `oi` searches after the global one: its `dlopen` group, breadth-first.
    fn group_scope(&self, root: usize) -> Vec<usize> {
        let mut out = vec![root];
        let mut q = 0;
        while q < out.len() {
            for &d in &self.objs[out[q]].deps {
                if !out.contains(&d) {
                    out.push(d);
                }
            }
            q += 1;
        }
        out
    }

    /// Resolve `name` (version `want`) in `scope`; the loader's own table first unless `skip_loader`.
    /// `Some((usize::MAX, addr, sym))` is a loader symbol.
    fn lookup(&mut self, scope: &[usize], name: &[u8], want: Option<&[u8]>, skip_loader: bool) -> Option<(usize, u64, Sym)> {
        self.stats.lookups += 1;
        if !skip_loader && self.tramp != 0 {
            if let Some(&(_, off)) = LOADER_SYMS.iter().find(|x| x.0 == name) {
                return Some((usize::MAX, self.tramp + off, Sym::default()));
            }
        }
        let gh = gnu_hash(name);
        let eh = elf_hash(name);
        for &oi in scope {
            if let Some(i) = self.find_in(oi, name, gh, eh, want) {
                let sym = Sym::at(&self.objs[oi].dynsym, i).unwrap_or_default();
                let addr = if sym.kind() == STT_TLS { sym.value } else { self.objs[oi].base + sym.value };
                return Some((oi, addr, sym));
            }
        }
        None
    }

    // ---------------------------------------------------------------------------------------------
    // relocation
    // ---------------------------------------------------------------------------------------------

    fn relocate<S: Space<File = F>>(&mut self, sp: &mut S, oi: usize) -> R<()> {
        if self.objs[oi].relocated {
            return Ok(());
        }
        let mut scope = self.global.clone();
        let g = self.objs[oi].group;
        if g != usize::MAX {
            for x in self.group_scope(g) {
                if !scope.contains(&x) {
                    scope.push(x);
                }
            }
        }
        let nsyms = self.objs[oi].dynsym.len() / 24;
        // Per symbol index: (defining object or usize::MAX for the loader / u32::MAX-ish for unresolved weak, value).
        let mut cache: Vec<Option<(usize, u64)>> = vec![None; nsyms];
        let base = self.objs[oi].base;
        let f = self.objs[oi].file.clone();
        let phdrs = self.objs[oi].phdrs.clone();
        let path = self.objs[oi].path.clone();
        let tables = [self.objs[oi].rela, self.objs[oi].jmprel];
        const CHUNK: u64 = 2730;
        let mut n = 0u64;
        for (va, size) in tables {
            if size == 0 {
                continue;
            }
            let off = file_off(&phdrs, va, size).ok_or_else(|| format!("{}: relocation table not in the file", s(&path)))?;
            let total = size / 24;
            let mut k = 0u64;
            while k < total {
                let cnt = (total - k).min(CHUNK);
                let buf = self.fread(sp, &f, off + k * 24, cnt * 24)?;
                for e in 0..cnt as usize {
                    let r_off = u64_at(&buf, e * 24).unwrap();
                    let info = u64_at(&buf, e * 24 + 8).unwrap();
                    let addend = u64_at(&buf, e * 24 + 16).unwrap();
                    let t = (info & 0xFFFF_FFFF) as u32;
                    let si = (info >> 32) as usize;
                    let at = base + r_off;
                    let val: u64 = match t {
                        R_X86_64_NONE => continue,
                        R_X86_64_RELATIVE => base.wrapping_add(addend),
                        R_X86_64_64 | R_X86_64_GLOB_DAT | R_X86_64_JUMP_SLOT | R_X86_64_DTPMOD64 | R_X86_64_DTPOFF64 | R_X86_64_TPOFF64 => {
                            let (def, sval) = if si == 0 {
                                (oi, 0)
                            } else {
                                match cache.get(si).copied().flatten() {
                                    Some(c) => c,
                                    None => {
                                        let c = self.resolve_sym(oi, si, &scope)?;
                                        if si < cache.len() {
                                            cache[si] = Some(c);
                                        }
                                        c
                                    }
                                }
                            };
                            match t {
                                R_X86_64_64 => sval.wrapping_add(addend),
                                R_X86_64_GLOB_DAT | R_X86_64_JUMP_SLOT => sval,
                                R_X86_64_DTPMOD64 => {
                                    // The loader's own __tls_get_addr reads this slot: the module's TP offset, not an id.
                                    let d = self.tls_def(def, &path)?;
                                    0u64.wrapping_sub(d)
                                }
                                R_X86_64_DTPOFF64 => {
                                    self.tls_def(def, &path)?;
                                    sval.wrapping_add(addend)
                                }
                                _ => {
                                    let d = self.tls_def(def, &path)?;
                                    sval.wrapping_add(addend).wrapping_sub(d)
                                }
                            }
                        }
                        other => {
                            return Err(format!("{}: relocation R_X86_64_{} ({}) refused — not in the probed set", s(&path), rel_name(other), other));
                        }
                    };
                    if !sp.write(at, &val.to_le_bytes()) {
                        return Err(format!("{}: relocation store at {:#x} failed", s(&path), at));
                    }
                    n += 1;
                }
                k += cnt;
            }
        }
        self.objs[oi].relocs = n;
        self.objs[oi].relocated = true;
        self.stats.relocs += n;
        Ok(())
    }

    /// The TLS distance of the module defining a TLS reference.
    fn tls_def(&self, def: usize, path: &[u8]) -> R<u64> {
        match self.objs.get(def) {
            Some(o) if o.tls.is_some() => Ok(o.tls_dist),
            _ => Err(format!("{}: TLS relocation against a module without PT_TLS", s(path))),
        }
    }

    /// Resolve symbol `si` of object `oi`: (defining object, value). TLS values are the symbol's offset in its block.
    fn resolve_sym(&mut self, oi: usize, si: usize, scope: &[usize]) -> R<(usize, u64)> {
        let o = &self.objs[oi];
        let sym = Sym::at(&o.dynsym, si).ok_or_else(|| format!("{}: symbol index {} out of range", s(&o.path), si))?;
        let name = cstr(&o.dynstr, sym.name as usize).to_vec();
        if sym.bind() == STB_LOCAL && sym.shndx != SHN_UNDEF {
            let v = if sym.kind() == STT_TLS { sym.value } else { o.base + sym.value };
            return Ok((oi, v));
        }
        let want = Self::want_version(o, si).map(|w| w.to_vec());
        match self.lookup(scope, &name, want.as_deref(), false) {
            Some((d, v, _)) => Ok((d, v)),
            None if sym.bind() == STB_WEAK => Ok((oi, 0)),
            None => Err(format!(
                "{}: undefined symbol {}{}{}",
                s(&self.objs[oi].path),
                s(&name),
                if want.is_some() { "@" } else { "" },
                want.as_deref().map_or("", s)
            )),
        }
    }

    // ---------------------------------------------------------------------------------------------
    // the arena (the loader's memory in the program: TLS template, lists, dl table, strings)
    // ---------------------------------------------------------------------------------------------

    fn alloc<S: Space<File = F>>(&mut self, sp: &mut S, len: u64, align: u64) -> R<u64> {
        let a = (self.arena.0 + align - 1) & !(align - 1);
        if self.arena.0 != 0 && a + len <= self.arena.1 {
            self.arena.0 = a + len;
            return Ok(a);
        }
        let chunk = page_up((len + align).max(64 * 1024));
        let at = sp.reserve(chunk, None)?;
        if !sp.map_anon(at, chunk, PROT_R | PROT_W) {
            return Err(String::from("loader arena mapping failed"));
        }
        self.stats.pages_mapped += chunk / PAGE;
        self.arena = (at, at + chunk);
        let a = (at + align - 1) & !(align - 1);
        self.arena.0 = a + len;
        Ok(a)
    }

    fn put<S: Space<File = F>>(&mut self, sp: &mut S, bytes: &[u8], align: u64) -> R<u64> {
        let a = self.alloc(sp, bytes.len().max(1) as u64, align)?;
        if !sp.write(a, bytes) {
            return Err(String::from("loader arena store failed"));
        }
        Ok(a)
    }

    fn put_cstr<S: Space<File = F>>(&mut self, sp: &mut S, b: &[u8]) -> R<u64> {
        let mut v = b.to_vec();
        v.push(0);
        self.put(sp, &v, 1)
    }

    fn put_list<S: Space<File = F>>(&mut self, sp: &mut S, fns: &[u64]) -> R<u64> {
        let mut v = Vec::with_capacity((fns.len() + 1) * 8);
        for f in fns.iter().chain(core::iter::once(&0u64)) {
            v.extend_from_slice(&f.to_le_bytes());
        }
        self.put(sp, &v, 8)
    }

    fn data_set<S: Space<File = F>>(&mut self, sp: &mut S, slot: u64, v: u64) -> R<()> {
        if !sp.write(self.data + slot, &v.to_le_bytes()) {
            return Err(String::from("trampoline data store failed"));
        }
        Ok(())
    }

    fn read_u64<S: Space<File = F>>(sp: &mut S, va: u64) -> u64 {
        let mut b = [0u8; 8];
        if sp.read(va, &mut b) { u64::from_le_bytes(b) } else { 0 }
    }

    /// Constructors of `order` (dependencies first): DT_INIT, then DT_INIT_ARRAY, as relocated in memory.
    fn ctor_list<S: Space<File = F>>(&mut self, sp: &mut S, order: &[usize]) -> Vec<u64> {
        let mut v = Vec::new();
        for &i in order {
            let o = &self.objs[i];
            if o.is_libc || o.inited {
                continue;
            }
            if o.init != 0 {
                v.push(o.init);
            }
            let (a, n) = o.init_array;
            for k in 0..n {
                let f = Self::read_u64(sp, a + k * 8);
                if f != 0 && f != u64::MAX {
                    v.push(f);
                }
            }
        }
        for &i in order {
            self.objs[i].inited = true;
        }
        v
    }

    /// Destructors, the reverse: per object (last first) DT_FINI_ARRAY backwards, then DT_FINI.
    fn dtor_list<S: Space<File = F>>(&self, sp: &mut S, order: &[usize]) -> Vec<u64> {
        let mut v = Vec::new();
        for &i in order.iter().rev() {
            let o = &self.objs[i];
            if o.is_libc {
                continue;
            }
            let (a, n) = o.fini_array;
            for k in (0..n).rev() {
                let f = Self::read_u64(sp, a + k * 8);
                if f != 0 && f != u64::MAX {
                    v.push(f);
                }
            }
            if o.fini != 0 {
                v.push(o.fini);
            }
        }
        v
    }

    /// Post-order (dependencies first) over the dependency graph from `root`.
    fn init_order(&self, root: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut seen = vec![false; self.objs.len()];
        let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
        seen[root] = true;
        while let Some(&mut (o, ref mut k)) = stack.last_mut() {
            if *k < self.objs[o].deps.len() {
                let d = self.objs[o].deps[*k];
                *k += 1;
                if !seen[d] {
                    seen[d] = true;
                    stack.push((d, 0));
                }
            } else {
                out.push(o);
                stack.pop();
            }
        }
        out
    }

    /// Write the TLS initial image of object `i` into the template (and, for a late object, the caller's block at `tp`).
    fn tls_image<S: Space<File = F>>(&mut self, sp: &mut S, i: usize, tp: Option<u64>) -> R<()> {
        let Some(m) = self.objs[i].tls else { return Ok(()) };
        let f = self.objs[i].file.clone();
        let img = self.fread(sp, &f, self.objs[i].tls_off, m.filesz)?;
        let at = self.tls_template + self.tls_size - self.objs[i].tls_dist;
        if !sp.write(at, &img) {
            return Err(String::from("TLS template store failed"));
        }
        if let Some(tp) = tp {
            let blk = tp.wrapping_sub(self.objs[i].tls_dist);
            let mut whole = img;
            whole.resize(m.memsz as usize, 0);
            if !sp.write(blk, &whole) {
                return Err(String::from("TLS block store (calling thread) failed"));
            }
        }
        Ok(())
    }

    /// Rewrite the `dl_iterate_phdr` table (64-byte `dl_phdr_info` records) and publish it.
    fn publish_table<S: Space<File = F>>(&mut self, sp: &mut S) -> R<()> {
        for i in 0..self.objs.len() {
            if self.objs[i].name_va == 0 {
                let nm = if i == 0 { Vec::new() } else { self.objs[i].path.clone() };
                self.objs[i].name_va = self.put_cstr(sp, &nm)?;
            }
        }
        let n = self.objs.len() as u64;
        let mut v = Vec::with_capacity(self.objs.len() * 64);
        for o in &self.objs {
            v.extend_from_slice(&o.base.to_le_bytes());
            v.extend_from_slice(&o.name_va.to_le_bytes());
            v.extend_from_slice(&o.phdr_va.to_le_bytes());
            v.extend_from_slice(&(o.phdrs.len() as u64).to_le_bytes());
            v.extend_from_slice(&n.to_le_bytes()); // dlpi_adds
            v.extend_from_slice(&0u64.to_le_bytes()); // dlpi_subs
            v.extend_from_slice(&o.tls_id.to_le_bytes());
            v.extend_from_slice(&0u64.to_le_bytes()); // dlpi_tls_data
        }
        let tab = self.put(sp, &v, 8)?;
        self.data_set(sp, DATA_OBJ_TAB, tab)?;
        self.data_set(sp, DATA_OBJ_N, n)
    }

    // ---------------------------------------------------------------------------------------------
    // the program
    // ---------------------------------------------------------------------------------------------

    /// Load the program at `path` and everything it needs; relocate; lay out TLS; build the trampoline and its tables.
    /// `gates` = the host runner's (dlopen, dlsym, dlclose, dladdr) functions; `None` = the kernel's syscall gates.
    pub fn load_program<S: Space<File = F>>(&mut self, sp: &mut S, path: &[u8], gates: Option<[u64; 4]>) -> R<Start> {
        let (f, size) = sp.open(path).ok_or_else(|| format!("{}: cannot open", s(path)))?;
        let exe = self.map_object(sp, path, f, size)?;
        if exe != 0 {
            return Err(String::from("load_program on a used loader"));
        }
        self.load_deps(sp, 0, usize::MAX)?;
        self.global = (0..self.objs.len()).collect();
        // TLS: the executable first, then load order.
        let mods: Vec<Option<tls::Mod>> = self.objs.iter().map(|o| o.tls).collect();
        let lay = tls::layout(&mods).ok_or("TLS layout refused (a module's alignment exceeds 64)")?;
        for (i, d) in lay.dist.iter().enumerate() {
            if let Some(d) = d {
                self.objs[i].tls_dist = *d;
                self.objs[i].tls_id = self.tls_next_id;
                self.tls_next_id += 1;
            }
        }
        self.tls_used = lay.used;
        self.tls_size = lay.size;
        // The trampoline: one code page, one data page.
        let at = sp.reserve(2 * PAGE, None)?;
        let mut code = vec![0xCCu8; PAGE as usize];
        code[..tramp_bytes::TRAMP_CODE.len()].copy_from_slice(&tramp_bytes::TRAMP_CODE);
        if !sp.map_code(at, &code) || !sp.map_anon(at + PAGE, PAGE, PROT_R | PROT_W) {
            return Err(String::from("trampoline mapping failed"));
        }
        self.stats.pages_mapped += 2;
        self.tramp = at;
        self.data = at + PAGE;
        // The TLS template and the synthetic program headers musl's static __init_tls reads.
        self.tls_template = self.alloc(sp, self.tls_size, PAGE)?;
        for i in 0..self.objs.len() {
            self.tls_image(sp, i, None)?;
        }
        let ph = self.alloc(sp, 3 * 56, 8)?;
        let mut t = Vec::with_capacity(3 * 56);
        let mut phdr = |ty: u32, flags: u32, va: u64, filesz: u64, memsz: u64, align: u64| {
            t.extend_from_slice(&ty.to_le_bytes());
            t.extend_from_slice(&flags.to_le_bytes());
            t.extend_from_slice(&0u64.to_le_bytes());
            t.extend_from_slice(&va.to_le_bytes());
            t.extend_from_slice(&va.to_le_bytes());
            t.extend_from_slice(&filesz.to_le_bytes());
            t.extend_from_slice(&memsz.to_le_bytes());
            t.extend_from_slice(&align.to_le_bytes());
        };
        phdr(PT_PHDR, PF_R, ph, 3 * 56, 3 * 56, 8);
        phdr(PT_TLS, PF_R, self.tls_template, self.tls_size, self.tls_size, tls::ALIGN);
        phdr(PT_GNU_STACK, PF_R | PF_W, 0, 0, 0, 16);
        if !sp.write(ph, &t) {
            return Err(String::from("synthetic phdr store failed"));
        }
        // libc: the object whose real __libc_start_main the trampoline's hook hands on to.
        let scope = self.global.clone();
        let (lc, start, _) = self
            .lookup(&scope, b"__libc_start_main", None, true)
            .ok_or("no __libc_start_main in any loaded object (is libc.so a DT_NEEDED?)")?;
        self.objs[lc].is_libc = true;
        let atexit = self.lookup(&scope, b"atexit", None, true).map_or(0, |x| x.1);
        // Relocate, dependencies first.
        let order = self.init_order(0);
        for &i in &order {
            self.relocate(sp, i)?;
        }
        let ctors = self.ctor_list(sp, &order);
        let dtors = self.dtor_list(sp, &order);
        let il = self.put_list(sp, &ctors)?;
        let fl = if dtors.is_empty() { 0 } else { self.put_list(sp, &dtors)? };
        self.data_set(sp, DATA_LIBC_START, start)?;
        self.data_set(sp, DATA_INIT_LIST, il)?;
        self.data_set(sp, DATA_FINI_LIST, fl)?;
        self.data_set(sp, DATA_ATEXIT, atexit)?;
        let g = gates.unwrap_or([
            self.tramp + tramp_bytes::T_GATE_OPEN,
            self.tramp + tramp_bytes::T_GATE_SYM,
            self.tramp + tramp_bytes::T_GATE_CLOSE,
            self.tramp + tramp_bytes::T_GATE_ADDR,
        ]);
        self.data_set(sp, DATA_G_OPEN, g[0])?;
        self.data_set(sp, DATA_G_SYM, g[1])?;
        self.data_set(sp, DATA_G_CLOSE, g[2])?;
        self.data_set(sp, DATA_G_ADDR, g[3])?;
        self.publish_table(sp)?;
        Ok(Start { entry: self.objs[0].entry, phdr: ph, phnum: 3, phent: 56, tramp: self.tramp })
    }

    fn set_err<S: Space<File = F>>(&mut self, sp: &mut S, msg: &str) {
        if let Ok(p) = self.put_cstr(sp, msg.as_bytes()) {
            let _ = self.data_set(sp, DATA_ERRSTR, p);
        }
    }

    /// `dlopen(path, flags)` from the thread whose thread pointer is `tp`. Returns the handle (0 = error, `dlerror` set).
    /// The new objects' constructors are left in the PENDING slot for the trampoline to run on return.
    pub fn dlopen<S: Space<File = F>>(&mut self, sp: &mut S, path: Option<&[u8]>, flags: u64, tp: u64) -> u64 {
        match self.dlopen_inner(sp, path, flags, tp) {
            Ok(h) => h,
            Err(e) => {
                self.set_err(sp, &e);
                0
            }
        }
    }

    fn dlopen_inner<S: Space<File = F>>(&mut self, sp: &mut S, path: Option<&[u8]>, flags: u64, tp: u64) -> R<u64> {
        self.stats.dlopens += 1;
        let Some(path) = path else { return Ok(1) }; // dlopen(NULL): the program (global scope)
        if let Some(i) = self.loaded(path) {
            self.objs[i].refs += 1;
            if flags & RTLD_GLOBAL != 0 && !self.global.contains(&i) {
                self.global.push(i);
            }
            return Ok(i as u64 + 1);
        }
        if flags & RTLD_NOLOAD != 0 {
            return Err(format!("{}: not loaded (RTLD_NOLOAD)", s(path)));
        }
        let (p, f, size) = self.find_lib(sp, path, 0).ok_or_else(|| format!("{}: cannot open shared object file", s(path)))?;
        let first = self.objs.len();
        let root = self.map_object(sp, &p, f, size)?;
        self.objs[root].group = root;
        self.load_deps(sp, root, root)?;
        // TLS for the new objects, from the surplus.
        for i in first..self.objs.len() {
            if let Some(m) = self.objs[i].tls {
                let d = tls::place(self.tls_used, &m).ok_or("TLS alignment refused")?;
                if d > self.tls_size {
                    return Err(format!("{}: static TLS surplus exhausted ({} of {} bytes)", s(&self.objs[i].path), d, self.tls_size));
                }
                self.tls_used = d;
                self.objs[i].tls_dist = d;
                self.objs[i].tls_id = self.tls_next_id;
                self.tls_next_id += 1;
                self.tls_image(sp, i, Some(tp))?;
            }
        }
        if flags & RTLD_GLOBAL != 0 {
            for i in self.group_scope(root) {
                if !self.global.contains(&i) {
                    self.global.push(i);
                }
            }
        }
        let order = self.init_order(root);
        for &i in &order {
            self.relocate(sp, i)?;
        }
        let ctors = self.ctor_list(sp, &order);
        let pending = if ctors.is_empty() { 0 } else { self.put_list(sp, &ctors)? };
        self.data_set(sp, DATA_PENDING, pending)?;
        self.publish_table(sp)?;
        Ok(root as u64 + 1)
    }

    /// `dlsym(handle, name)`. 0 = not found (`dlerror` set).
    pub fn dlsym<S: Space<File = F>>(&mut self, sp: &mut S, handle: u64, name: &[u8]) -> u64 {
        let scope = if handle == RTLD_DEFAULT || handle == RTLD_NEXT || handle == 1 {
            self.global.clone()
        } else {
            match (handle as usize).checked_sub(1).filter(|&i| i < self.objs.len()) {
                Some(i) => self.group_scope(i),
                None => {
                    self.set_err(sp, "dlsym: invalid handle");
                    return 0;
                }
            }
        };
        match self.lookup(&scope, name, None, false) {
            Some((d, v, sym)) if d == usize::MAX || sym.kind() != STT_TLS => v,
            Some(_) => {
                self.set_err(sp, &format!("dlsym: {} is a TLS symbol (not served)", s(name)));
                0
            }
            None => {
                self.set_err(sp, &format!("dlsym: symbol {} not found", s(name)));
                0
            }
        }
    }

    /// `dlclose(handle)`: the reference drops; the object stays mapped (stated, owed).
    pub fn dlclose<S: Space<File = F>>(&mut self, sp: &mut S, handle: u64) -> u64 {
        match (handle as usize).checked_sub(1).and_then(|i| self.objs.get_mut(i)) {
            Some(o) => {
                o.refs = o.refs.saturating_sub(1);
                0
            }
            None => {
                self.set_err(sp, "dlclose: invalid handle");
                u64::MAX
            }
        }
    }

    /// `dladdr(addr, info)`: fills `Dl_info {dli_fname, dli_fbase, dli_sname, dli_saddr}` at `info`. 1 = found.
    pub fn dladdr<S: Space<File = F>>(&mut self, sp: &mut S, addr: u64, info: u64) -> u64 {
        let Some(i) = self.objs.iter().position(|o| addr >= o.lo && addr < o.hi) else { return 0 };
        if self.objs[i].name_va == 0 && self.publish_table(sp).is_err() {
            return 0;
        }
        let o = &self.objs[i];
        // The nearest defined symbol at or below addr.
        let (mut best, mut best_v) = (None, 0u64);
        for k in 0..o.dynsym.len() / 24 {
            if let Some(sym) = Sym::at(&o.dynsym, k) {
                if sym.shndx == SHN_UNDEF || sym.kind() == STT_TLS {
                    continue;
                }
                let v = o.base + sym.value;
                if v <= addr && v >= best_v && (sym.size == 0 || addr < v + sym.size || best.is_none()) {
                    best = Some(sym);
                    best_v = v;
                }
            }
        }
        let fname = o.name_va;
        let fbase = if o.ty == ET_EXEC { o.lo } else { o.base };
        let (sname, saddr) = match best {
            Some(sym) => (o.strtab_va + sym.name as u64, best_v),
            None => (0, 0),
        };
        // The executable's dl_iterate_phdr name is "" (as glibc's); dladdr names it by its path.
        let fname = if i == 0 {
            let p = self.objs[0].path.clone();
            match self.put_cstr(sp, &p) {
                Ok(a) => a,
                Err(_) => fname,
            }
        } else {
            fname
        };
        let mut v = Vec::with_capacity(32);
        for w in [fname, fbase, sname, saddr] {
            v.extend_from_slice(&w.to_le_bytes());
        }
        if sp.write(info, &v) { 1 } else { 0 }
    }

    /// The program's path, for dladdr of the executable (the record's name stays "" as glibc's does).
    pub fn exe_path(&self) -> &[u8] {
        self.objs.first().map_or(&[], |o| &o.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes() {
        assert_eq!(gnu_hash(b""), 5381);
        assert_eq!(gnu_hash(b"printf"), 0x156b_2bb8);
        assert_eq!(elf_hash(b"printf"), 0x0779_05a6);
        assert_eq!(elf_hash(b"GCC_3.0"), 0x0b79_2650);
    }

    #[test]
    fn paths() {
        assert_eq!(basename(b"/a/b/c.so"), b"c.so");
        assert_eq!(dirname(b"/a/b/c.so"), b"/a/b");
        assert_eq!(dirname(b"c.so"), b".");
    }

    #[test]
    fn loader_syms_point_into_the_page() {
        for (_, off) in LOADER_SYMS {
            assert!((off as usize) < tramp_bytes::TRAMP_CODE.len());
        }
    }
}
