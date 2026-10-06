// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Kernel — shared-core
//!
//! SELFBUILD6 (rmbp-ledger B360) — the Linux ABI shim's DYNAMIC programs. A program with `PT_INTERP` (`/lib/ld-musl-x86_64.so.1`)
//! is not handed to musl's `ld.so` (C; R79): the kernel fulfils `ldso_core::Space` over the process's VMAs and runs the ONE
//! loader core the host runner (`ldso_core`'s `ldrun`) also runs — map the program and every `DT_NEEDED` object, resolve,
//! relocate, lay out static TLS, build the ring-3 trampoline page and its tables — then enters the program with the loader's
//! synthetic `AT_PHDR` (one `PT_TLS` musl's static start-up reads).
//!
//! * Shared objects and PIEs carry no fixed address: first fit in the 448 GiB second mmap window (`vm::MMAP2_*`), as lazy
//!   MAP_PRIVATE file VMAs (SELFBUILD4's fault hook pages them in; a relocation store faults its page in through
//!   `copy_out`). ASLR is OFF.
//! * An `ET_EXEC` (the musl `rust-lld`, 0x400000..0x8d34000) must fit the image window `elf.rs` enforces; outside it the load
//!   answers `skip(window)` (B361 WINDOW2 raises the window; 141 MB does not fit 64 MiB either).
//! * `dlopen` / `dlsym` / `dlclose` / `dladdr` reach [`gate`] through the trampoline's private syscalls 7504..7507; the
//!   loader state lives per address space (keyed by PML4), copied at fork, replaced at execve, dropped with the session.
//! * Search path: `DT_RPATH`/`DT_RUNPATH` (with `$ORIGIN`), then [`LIB_DIRS`] (where arroyo stages `libc.so` and
//!   `libgcc_s.so.1`, relinked from the Rust musl target's self-contained `libc.a` / `libunwind.a`).

use super::vm::{self, Vma};
use super::{elf, AddrSpace, LinuxProc, PAGE};
use super::proc::ProcInfo;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use ldso_core::{Loader, Space, PROT_R, PROT_W, PROT_X};

/// Where the relinked `libc.so` / `libgcc_s.so.1` (and the dyn probe) are staged.
pub const LIB_DIRS: [&str; 1] = ["/lib/dyn"];
/// The fixed-address image window (follows `elf.rs` IMAGE_FLOOR / IMAGE_LIMIT; B361 WINDOW2 raises the limit).
const EXEC_FLOOR: u64 = 0x1_0000;
const EXEC_LIMIT: u64 = super::elf::IMAGE_LIMIT; // WINDOW2 (B361): follows the raised window, was 16 MiB

// ---- counters (the `tests selfbuild6` lines; the LAST dynamic load) ----
pub static LOADS: AtomicU64 = AtomicU64::new(0);
pub static LAST_RELOCS: AtomicU64 = AtomicU64::new(0);
pub static LAST_PAGES: AtomicU64 = AtomicU64::new(0);
pub static LAST_OBJECTS: AtomicU64 = AtomicU64::new(0);
pub static LAST_LOAD_MS: AtomicU64 = AtomicU64::new(0);
pub static LAST_READ: AtomicU64 = AtomicU64::new(0);
pub static DLOPENS: AtomicU64 = AtomicU64::new(0);
pub static DLSYMS: AtomicU64 = AtomicU64::new(0);
/// 1 = the last refusal was the image window (`skip(window)`).
pub static LAST_WINDOW_REFUSAL: AtomicU64 = AtomicU64::new(0);

type KLoader = Loader<Arc<String>>;

/// The loader of every dynamic address space of the session, by PML4.
static LOADERS: spin::Mutex<BTreeMap<u64, KLoader>> = spin::Mutex::new(BTreeMap::new());

/// `ldso_core::Space` over one process's address space.
pub struct KSpace<'a> {
    pub asp: &'a mut AddrSpace,
}

impl KSpace<'_> {
    fn vma(&mut self, va: u64, len: u64, prot: u32, file: Option<Arc<String>>, foff: u64) -> bool {
        let (r, w, x) = (prot & PROT_R != 0, prot & PROT_W != 0, prot & PROT_X != 0);
        if w && x || len == 0 || va & (PAGE - 1) != 0 || len & (PAGE - 1) != 0 {
            return false;
        }
        self.asp.vm.remove(va, va + len);
        self.asp.vm.insert(Vma { start: va, end: va + len, r, w, x, shared: false, file, foff });
        true
    }

    /// Make every page of `[va, va+len)` resident (read-populate), so a loader store into a read-only or not-yet-backed
    /// page lands (`copy_out(force)` populates for WRITE, which a read-only VMA refuses).
    fn back(&mut self, va: u64, len: u64) {
        let mut a = va & !(PAGE - 1);
        while a < va + len {
            if !self.asp.is_mapped(a) {
                let _ = self.asp.populate(a, false, false);
            }
            a += PAGE;
        }
    }
}

impl Space for KSpace<'_> {
    type File = Arc<String>;

    fn open(&mut self, path: &[u8]) -> Option<(Arc<String>, u64)> {
        use crate::fs::vfs::NodeKind;
        let p = core::str::from_utf8(path).ok()?;
        let full = crate::shell::vfs_path(p);
        let st = crate::shell::vfs_mount_table().stat(&full).ok()?;
        if matches!(st.kind, NodeKind::Dir) {
            return None;
        }
        Some((Arc::new(full), st.size))
    }

    fn read_file(&mut self, f: &Arc<String>, off: u64, buf: &mut [u8]) -> bool {
        let mt = crate::shell::vfs_mount_table();
        let mut done = 0usize;
        while done < buf.len() {
            let want = (buf.len() - done).min(1 << 20);
            let Ok(b) = mt.read(f, off + done as u64, want) else { return false };
            if b.is_empty() || b.len() > want {
                return false;
            }
            buf[done..done + b.len()].copy_from_slice(&b);
            done += b.len();
        }
        LAST_READ.fetch_add(buf.len() as u64, Ordering::Relaxed);
        true
    }

    fn reserve(&mut self, len: u64, fixed: Option<u64>) -> Result<u64, String> {
        match fixed {
            Some(a) => {
                let end = a.checked_add(len).ok_or_else(|| String::from("fixed range overflows"))?;
                if a < EXEC_FLOOR || end > EXEC_LIMIT {
                    LAST_WINDOW_REFUSAL.store(1, Ordering::Relaxed);
                    return Err(alloc::format!(
                        "skip(window): ET_EXEC at {:#x}..{:#x} is outside the image window {:#x}..{:#x}",
                        a, end, EXEC_FLOOR, EXEC_LIMIT
                    ));
                }
                if !super::memory::region_is_usable(a, len) {
                    return Err(String::from("fixed range is not free RAM on this machine (would overlay the kernel)"));
                }
                if self.asp.vm.overlaps(a, end) {
                    return Err(String::from("fixed range already mapped"));
                }
                Ok(a)
            }
            None => self
                .asp
                .vm
                .gap(len, vm::MMAP2_BASE, vm::MMAP2_LIMIT)
                .ok_or_else(|| alloc::format!("no room for {} bytes in the mmap window", len)),
        }
    }

    fn map_file(&mut self, va: u64, len: u64, f: &Arc<String>, off: u64, prot: u32) -> bool {
        self.vma(va, len, prot, Some(f.clone()), off)
    }

    fn map_anon(&mut self, va: u64, len: u64, prot: u32) -> bool {
        self.vma(va, len, prot, None, 0)
    }

    fn map_code(&mut self, va: u64, bytes: &[u8]) -> bool {
        if !self.vma(va, PAGE, PROT_R | PROT_X, None, 0) {
            return false;
        }
        self.back(va, PAGE);
        self.asp.copy_out(va, bytes, true)
    }

    fn write(&mut self, va: u64, data: &[u8]) -> bool {
        if self.asp.copy_out(va, data, true) {
            return true;
        }
        self.back(va, data.len() as u64);
        self.asp.copy_out(va, data, true)
    }

    fn read(&mut self, va: u64, out: &mut [u8]) -> bool {
        self.asp.copy_in(va, out)
    }
}

/// Does the image at `full` (a VFS path) ask for an interpreter? Only those take the loader path; a static-PIE (ET_DYN, no
/// PT_INTERP) is still refused by `elf.rs`, as before.
pub fn wants(full: &str) -> bool {
    let mt = crate::shell::vfs_mount_table();
    let Ok(head) = mt.read(full, 0, PAGE as usize) else { return false };
    let Ok(eh) = ldso_core::elf::parse(&head, u64::MAX) else { return false };
    eh.phdrs.iter().any(|p| p.ty == ldso_core::elf::PT_INTERP)
}

/// Load the dynamic program at `full` into `asp` (empty, or just `reset`). Returns the plan `elf::build_stack` takes: the entry
/// and the loader's synthetic program headers.
pub fn load(asp: &mut AddrSpace, full: &str) -> Result<elf::Plan, String> {
    let t0 = crate::arch::ms();
    LAST_READ.store(0, Ordering::Relaxed);
    LAST_WINDOW_REFUSAL.store(0, Ordering::Relaxed);
    let pml4 = asp.pml4;
    let mut l: KLoader = Loader::new(LIB_DIRS.iter().map(|d| Vec::from(d.as_bytes())).collect());
    let st = {
        let mut sp = KSpace { asp };
        l.load_program(&mut sp, full.as_bytes(), None)?
    };
    LOADS.fetch_add(1, Ordering::Relaxed);
    LAST_RELOCS.store(l.stats.relocs, Ordering::Relaxed);
    LAST_PAGES.store(l.stats.pages_mapped, Ordering::Relaxed);
    LAST_OBJECTS.store(l.stats.objects, Ordering::Relaxed);
    LAST_LOAD_MS.store(crate::arch::ms().saturating_sub(t0), Ordering::Relaxed);
    serial_println!(
        "[ldso] load path={} objects={} pages_mapped={} relocs={} lookups={} read={} ms={} entry={:#x} tramp={:#x}",
        full,
        l.stats.objects,
        l.stats.pages_mapped,
        l.stats.relocs,
        l.stats.lookups,
        LAST_READ.load(Ordering::Relaxed),
        LAST_LOAD_MS.load(Ordering::Relaxed),
        st.entry,
        st.tramp
    );
    for o in &l.objs {
        serial_println!(
            "[ldso]   {} base={:#x} span={} relocs={} tls={}",
            core::str::from_utf8(&o.path).unwrap_or("?"),
            o.base,
            o.hi - o.lo,
            o.relocs,
            if o.tls.is_some() { o.tls_dist as i64 } else { -1 }
        );
    }
    LOADERS.lock().insert(pml4, l);
    Ok(elf::Plan { entry: st.entry, segs: Vec::new(), phdr_va: st.phdr, phnum: st.phnum, phent: st.phent, pie: false }) // WINDOW2 (B361) added `pie`; ldso already placed and relocated every object, so the static-PIE path does not apply
}

/// A fork child inherits its parent's loader (same objects at the same addresses, copy-on-write).
pub fn fork_copy(parent: u64, child: u64) {
    let mut m = LOADERS.lock();
    if let Some(l) = m.get(&parent).cloned() {
        m.insert(child, l);
    }
}

/// The address space is gone (session end) or re-imaged statically (execve): drop its loader.
pub fn forget(pml4: u64) {
    LOADERS.lock().remove(&pml4);
}

/// The trampoline's gates: 7504 dlopen(path, flags), 7505 dlsym(handle, name), 7506 dlclose(handle), 7507 dladdr(addr, info).
/// The process lock is held (sys::handle), so the gates of one process are serialised.
pub fn gate(p: &mut LinuxProc, info: &Arc<ProcInfo>, nr: u64, a: [u64; 6]) -> i64 {
    let mut m = LOADERS.lock();
    let Some(l) = m.get_mut(&info.pml4) else { return -38 };
    let mut sp = KSpace { asp: &mut p.asp };
    let r = match nr {
        ldso_core::SYS_DLOPEN => {
            DLOPENS.fetch_add(1, Ordering::Relaxed);
            let path = if a[0] == 0 { None } else { sp.asp.read_cstr(a[0], 4096) };
            if a[0] != 0 && path.is_none() {
                return -14;
            }
            let tp = x86_64::registers::model_specific::FsBase::read().as_u64();
            let h = l.dlopen(&mut sp, path.as_deref(), a[1], tp);
            serial_println!(
                "[ldso] dlopen {} flags={:#x} -> {} objects={}",
                path.as_deref().map_or("(null)", |p| core::str::from_utf8(p).unwrap_or("?")),
                a[1],
                h,
                l.objs.len()
            );
            h
        }
        ldso_core::SYS_DLSYM => {
            DLSYMS.fetch_add(1, Ordering::Relaxed);
            let Some(name) = sp.asp.read_cstr(a[1], 4096) else { return -14 };
            l.dlsym(&mut sp, a[0], &name)
        }
        ldso_core::SYS_DLCLOSE => l.dlclose(&mut sp, a[0]),
        ldso_core::SYS_DLADDR => l.dladdr(&mut sp, a[0], a[1]),
        _ => return -38,
    };
    r as i64
}
