// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Kernel — driver
//!
//! SELFBUILD4 (B356) — LAZY execve images for the Linux ABI shim (the compatibility box, as SELFBUILD3's `vm.rs`).
//!
//! Before this arc `linux` and `execve` read the WHOLE ELF into the kernel heap (128 MiB cap) and copied every segment
//! into eagerly allocated pages. Now only the image's head (the ELF header and the program-header table) is read, and
//! each `PT_LOAD` becomes VMAs — the SELFBUILD3 fault hook pages the image in from the file on first touch:
//!
//! * a page wholly inside ONE segment's file bytes = a MAP_PRIVATE file VMA at that segment's offset (lazy);
//! * a page wholly past a segment's file bytes (its .bss) = an anonymous VMA (lazy, zero);
//! * every other page — the page holding a segment's first byte when the segment does not start on a page, the page
//!   holding the .data tail and the .bss head (the file bytes past `p_filesz` there are other sections, never .bss), or a
//!   page two segments share — is EAGER: allocated now and filled with exactly the segments' file bytes.
//!
//! The image window stays `0x10000..16 MiB` in PML4[0] (checked to be free RAM, never the kernel); until a page is
//! touched the process's tables there are the kernel's identity map (present, not USER), which the fault hook treats as
//! not-present for a VMA page. Pages already USER there would be someone else's: the load refuses them.

use super::{elf, AddrSpace, PAGE};
use super::vm::Vma;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

/// The head read at exec: enough for the ELF header and a program-header table that starts in the first page.
const HEAD: u64 = 4096;
/// A program-header table further in is read up to here.
const HEAD_MAX: u64 = 65536;

// ---- counters (the `tests selfbuild4` exec line; the LAST image loaded) ----
pub static LAZY_EXECS: AtomicU64 = AtomicU64::new(0);
pub static LAST_FILE_BYTES: AtomicU64 = AtomicU64::new(0);
pub static LAST_READ_BYTES: AtomicU64 = AtomicU64::new(0);
pub static LAST_LAZY_PAGES: AtomicU64 = AtomicU64::new(0);
pub static LAST_EAGER_PAGES: AtomicU64 = AtomicU64::new(0);
pub static LAST_VMAS: AtomicU64 = AtomicU64::new(0);

/// An image ready to load: its VFS path, the plan, the file size and the head bytes read.
pub struct Image {
    pub full: String,
    pub plan: elf::Plan,
    pub size: u64,
    pub read: u64,
}

/// Resolve `path`, read the head only, parse it. Errors read as `read_image`'s did (`-ENOENT`, `-EISDIR`, `-E2BIG`, `-EIO`).
pub fn image(path: &str) -> Result<Image, String> {
    use crate::fs::vfs::NodeKind;
    let full = crate::shell::vfs_path(path);
    let mt = crate::shell::vfs_mount_table();
    let st = mt.stat(&full).map_err(|_| alloc::format!("{}: no such file (-ENOENT)", full))?;
    if matches!(st.kind, NodeKind::Dir) {
        return Err(alloc::format!("{}: is a directory (-EISDIR)", full));
    }
    if st.size < 64 {
        return Err(alloc::format!("{}: size {} out of range (-E2BIG)", full, st.size));
    }
    let mut want = st.size.min(HEAD);
    let mut head = mt.read(&full, 0, want as usize).map_err(|_| alloc::format!("{}: read failed (-EIO)", full))?;
    if (head.len() as u64) < want.min(64) {
        return Err(alloc::format!("{}: short read (-EIO)", full));
    }
    // A program-header table past the first page: read up to its end (bounded).
    if head.len() >= 64 {
        let phoff = u64::from_le_bytes(head[32..40].try_into().unwrap_or([0; 8]));
        let phnum = u16::from_le_bytes([head[56], head[57]]) as u64;
        let end = phoff.saturating_add(phnum * 56);
        if end > head.len() as u64 && end <= st.size.min(HEAD_MAX) {
            want = end;
            head = mt.read(&full, 0, want as usize).map_err(|_| alloc::format!("{}: read failed (-EIO)", full))?;
        }
    }
    let read = head.len() as u64;
    let plan = elf::parse_sized(&head, st.size).map_err(String::from)?;
    Ok(Image { full, plan, size: st.size, read })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    File(usize),
    Anon(usize),
    Eager,
}

/// Map `img` into `asp` (empty, or just `reset`): lazy VMAs + the eager boundary pages (module note).
pub fn load(asp: &mut AddrSpace, img: &Image) -> Result<(), &'static str> {
    let segs = &img.plan.segs;
    // Every image page -> (writable, executable, segments touching it).
    let mut pages: BTreeMap<u64, (bool, bool, Vec<usize>)> = BTreeMap::new();
    for (i, s) in segs.iter().enumerate() {
        let (w, x) = (s.flags & 2 != 0, s.flags & 1 != 0);
        let mut a = s.vaddr & !(PAGE - 1);
        let end = (s.vaddr + s.memsz + PAGE - 1) & !(PAGE - 1);
        while a < end {
            let e = pages.entry(a).or_insert((false, false, Vec::new()));
            e.0 |= w;
            e.1 |= x;
            e.2.push(i);
            a += PAGE;
        }
    }
    let (Some(&lo), Some(&last)) = (pages.keys().next(), pages.keys().next_back()) else { return Err("no PT_LOAD segments") };
    if !super::memory::region_is_usable(lo, last + PAGE - lo) {
        return Err("load range is not free RAM on this machine (would overlay the kernel)");
    }
    let classify = |va: u64, segs_here: &[usize]| -> Kind {
        if segs_here.len() != 1 {
            return Kind::Eager;
        }
        let i = segs_here[0];
        let s = &segs[i];
        let file_end = s.vaddr + s.filesz;
        if va >= s.vaddr && va + PAGE <= file_end {
            Kind::File(i)
        } else if va >= file_end {
            Kind::Anon(i)
        } else {
            Kind::Eager
        }
    };
    let file = Arc::new(img.full.clone());
    let mt = crate::shell::vfs_mount_table();
    let (mut lazy, mut eager, mut vmas) = (0u64, 0u64, 0u64);
    let mut run: Option<(Kind, u64, u64)> = None; // (kind, start, end)
    let flush_run = |asp: &mut AddrSpace, run: &mut Option<(Kind, u64, u64)>, vmas: &mut u64| {
        if let Some((k, start, end)) = run.take() {
            let v = match k {
                Kind::File(i) => {
                    let s = &segs[i];
                    Vma { start, end, r: true, w: s.flags & 2 != 0, x: s.flags & 1 != 0, shared: false, file: Some(file.clone()), foff: s.off + (start - s.vaddr) }
                }
                Kind::Anon(i) => {
                    let s = &segs[i];
                    Vma { start, end, r: true, w: s.flags & 2 != 0, x: s.flags & 1 != 0, shared: false, file: None, foff: 0 }
                }
                Kind::Eager => return,
            };
            asp.vm.insert(v);
            *vmas += 1;
        }
    };
    for (&va, (w, x, here)) in &pages {
        if *w && *x {
            return Err("segments share a page with conflicting W/X");
        }
        let k = classify(va, here);
        if k != Kind::Eager && asp.is_mapped(va) {
            return Err("image page already mapped USER (not free for this process)");
        }
        match run {
            Some((rk, _, ref mut end)) if rk == k && *end == va && k != Kind::Eager => *end = va + PAGE,
            _ => {
                flush_run(asp, &mut run, &mut vmas);
                if k != Kind::Eager {
                    run = Some((k, va, va + PAGE));
                }
            }
        }
        if k == Kind::Eager {
            if !asp.map_new(va, *w, *x) {
                return Err("out of pages mapping the image");
            }
            for &i in here {
                let s = &segs[i];
                let (a, b) = (va.max(s.vaddr), (va + PAGE).min(s.vaddr + s.filesz));
                if a >= b {
                    continue;
                }
                let bytes = mt.read(&img.full, s.off + (a - s.vaddr), (b - a) as usize).map_err(|_| "image read failed")?;
                if bytes.len() as u64 != b - a || !asp.copy_out(a, &bytes, true) {
                    return Err("image read failed");
                }
                LAST_READ_BYTES.fetch_add(b - a, Ordering::Relaxed);
            }
            eager += 1;
        } else {
            lazy += 1;
        }
    }
    flush_run(asp, &mut run, &mut vmas);
    LAZY_EXECS.fetch_add(1, Ordering::Relaxed);
    LAST_FILE_BYTES.store(img.size, Ordering::Relaxed);
    LAST_LAZY_PAGES.store(lazy, Ordering::Relaxed);
    LAST_EAGER_PAGES.store(eager, Ordering::Relaxed);
    LAST_VMAS.store(vmas, Ordering::Relaxed);
    Ok(())
}

/// Reset the per-image read counter (the head) before a load; `load` adds the eager pages' bytes.
pub fn begin(img: &Image) {
    LAST_READ_BYTES.store(img.read, Ordering::Relaxed);
}
