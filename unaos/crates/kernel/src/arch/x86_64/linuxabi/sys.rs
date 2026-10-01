// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! LINUXABI M2 — the Linux x86_64 syscall table (the first ~40 numbers a musl-static program touches).
//!
//! Every user pointer goes through `AddrSpace::copy_in`/`copy_out` (software page-table walk, no kernel
//! deref of a user VA). Files are READ-ONLY and slurped whole at `open` through the shell's mount table.
//! Unknown numbers answer `-ENOSYS` with a once-per-number `[linuxabi] enosys nr=` line.

use super::{Fd, LinuxProc, BRK_BASE, BRK_MAX, MMAP_BASE, MMAP_LIMIT, PAGE, STACK_PAGES, STACK_TOP};
use alloc::string::String;
use alloc::vec::Vec;

const ENOENT: i64 = 2;
const EBADF: i64 = 9;
const ENOMEM: i64 = 12;
const EACCES: i64 = 13;
const EFAULT: i64 = 14;
const EINVAL: i64 = 22;
const EMFILE: i64 = 24;
const ENOTTY: i64 = 25;
const ESPIPE: i64 = 29;
const ENOSYS: i64 = 38;
const AT_FDCWD: i64 = -100;
const AT_EMPTY_PATH: u64 = 0x1000;
const MAX_FILE: u64 = 128 << 20;

fn page_up(v: u64) -> Option<u64> {
    v.checked_add(PAGE - 1).map(|x| x & !(PAGE - 1))
}

fn fd_slot(p: &mut LinuxProc, fd: u64) -> Option<&mut Fd> {
    p.fds.get_mut(fd as usize).and_then(|s| s.as_mut())
}

fn console_write(p: &LinuxProc, buf: u64, len: u64) -> i64 {
    let len = len.min(1 << 20);
    let mut done = 0u64;
    let mut chunk = [0u8; 1024];
    while done < len {
        let n = ((len - done) as usize).min(chunk.len());
        if !p.asp.copy_in(buf + done, &mut chunk[..n]) {
            return if done > 0 { done as i64 } else { -EFAULT };
        }
        serial_print!("{}", String::from_utf8_lossy(&chunk[..n]));
        done += n as u64;
    }
    done as i64
}

fn load_file(path: &str) -> Result<(Vec<u8>, bool), i64> {
    use crate::fs::vfs::NodeKind;
    let full = crate::shell::vfs_path(path);
    let mt = crate::shell::vfs_mount_table();
    let st = mt.stat(&full).map_err(|_| -ENOENT)?;
    if matches!(st.kind, NodeKind::Dir) {
        return Ok((Vec::new(), true));
    }
    if st.size > MAX_FILE {
        return Err(-ENOMEM);
    }
    if st.size == 0 {
        return Ok((Vec::new(), false));
    }
    let data = mt.read(&full, 0, st.size as usize).map_err(|_| -5i64)?;
    Ok((data, false))
}

fn do_open(p: &mut LinuxProc, dirfd: i64, path_va: u64, flags: u64) -> i64 {
    let Some(raw) = p.asp.read_cstr(path_va, 4096) else { return -EFAULT };
    let Ok(path) = core::str::from_utf8(&raw) else { return -EINVAL };
    if flags & 3 != 0 {
        return -EACCES; // read-only filesystem view in rung 1
    }
    let mut full = String::from(path);
    if !path.starts_with('/') && dirfd != AT_FDCWD {
        let _ = dirfd; // dirfd-relative resolution needs the directory's path; cwd-relative only
    }
    if !full.starts_with('/') {
        let mut c = p.cwd.clone();
        if !c.ends_with('/') {
            c.push('/');
        }
        c.push_str(&full);
        full = c;
    }
    let (data, dir) = match load_file(&full) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let f = Fd::File { data, pos: 0, dir };
    for (i, s) in p.fds.iter_mut().enumerate().skip(3) {
        if s.is_none() {
            *s = Some(f);
            return i as i64;
        }
    }
    if p.fds.len() >= 64 {
        return -EMFILE;
    }
    p.fds.push(Some(f));
    (p.fds.len() - 1) as i64
}

fn put_stat(p: &LinuxProc, buf: u64, mode: u32, size: u64) -> i64 {
    let mut s = [0u8; 144];
    s[16..24].copy_from_slice(&1u64.to_le_bytes()); // st_nlink
    s[24..28].copy_from_slice(&mode.to_le_bytes());
    s[48..56].copy_from_slice(&size.to_le_bytes());
    s[56..64].copy_from_slice(&4096u64.to_le_bytes()); // st_blksize
    s[64..72].copy_from_slice(&size.div_ceil(512).to_le_bytes()); // st_blocks
    if p.asp.copy_out(buf, &s, false) { 0 } else { -EFAULT }
}

fn fstat(p: &mut LinuxProc, fd: u64, buf: u64) -> i64 {
    let (mode, size) = match fd_slot(p, fd) {
        None => return -EBADF,
        Some(Fd::Console) => (0o020620u32, 0u64),
        Some(Fd::File { data, dir, .. }) => (if *dir { 0o040555 } else { 0o100444 }, data.len() as u64),
    };
    put_stat(p, buf, mode, size)
}

fn mmap(p: &mut LinuxProc, addr: u64, len: u64, prot: u64, flags: u64, fd: u64, off: u64) -> i64 {
    if len == 0 {
        return -EINVAL;
    }
    let Some(len) = page_up(len) else { return -ENOMEM };
    let (w, x) = (prot & 2 != 0, prot & 4 != 0);
    if w && x {
        return -EACCES; // W^X
    }
    let anon = flags & 0x20 != 0;
    if !anon && (flags & 3 == 1) && w {
        return -EACCES; // shared writable file mapping: no write-back in rung 1
    }
    let stack_lo = STACK_TOP - STACK_PAGES * PAGE;
    let base = if flags & 0x10 != 0 {
        if addr & (PAGE - 1) != 0 || addr < MMAP_BASE || addr.checked_add(len).map_or(true, |e| e > stack_lo) {
            return -ENOMEM;
        }
        let mut a = addr;
        while a < addr + len {
            p.asp.unmap(a);
            a += PAGE;
        }
        addr
    } else {
        let b = p.mmap_next;
        if b.checked_add(len).map_or(true, |e| e > MMAP_LIMIT) {
            return -ENOMEM;
        }
        p.mmap_next = b + len;
        b
    };
    let mut done = 0u64;
    while done < len {
        if !p.asp.map_new(base + done, w, x) {
            let mut a = base;
            while a < base + done {
                p.asp.unmap(a);
                a += PAGE;
            }
            return -ENOMEM;
        }
        done += PAGE;
    }
    if !anon {
        if off & (PAGE - 1) != 0 {
            return -EINVAL;
        }
        // Copy while still writable-by-kernel (`force`); the PTE perms are what ring 3 sees.
        let Some(Fd::File { data, .. }) = fd_slot(p, fd) else { return -EBADF };
        let start = (off as usize).min(data.len());
        let end = (start + len as usize).min(data.len());
        let chunk: Vec<u8> = data[start..end].to_vec();
        if !p.asp.copy_out(base, &chunk, true) {
            return -EFAULT;
        }
    }
    flush();
    base as i64
}

fn munmap(p: &mut LinuxProc, addr: u64, len: u64) -> i64 {
    if addr & (PAGE - 1) != 0 || len == 0 {
        return -EINVAL;
    }
    let Some(len) = page_up(len) else { return -EINVAL };
    let mut a = addr;
    while a < addr.saturating_add(len) {
        p.asp.unmap(a);
        a += PAGE;
    }
    flush();
    0
}

fn mprotect(p: &mut LinuxProc, addr: u64, len: u64, prot: u64) -> i64 {
    if addr & (PAGE - 1) != 0 {
        return -EINVAL;
    }
    let Some(len) = page_up(len) else { return -ENOMEM };
    let (w, x) = (prot & 2 != 0, prot & 4 != 0);
    if w && x {
        return -EACCES;
    }
    let mut a = addr;
    while a < addr.saturating_add(len) {
        if !p.asp.is_mapped(a) {
            return -ENOMEM;
        }
        a += PAGE;
    }
    let mut a = addr;
    while a < addr + len {
        p.asp.set_perms(a, w, x);
        a += PAGE;
    }
    flush();
    0
}

fn brk(p: &mut LinuxProc, want: u64) -> i64 {
    if want < BRK_BASE || want > BRK_BASE + BRK_MAX {
        return p.brk as i64;
    }
    let Some(end) = page_up(want) else { return p.brk as i64 };
    while p.brk_mapped < end {
        if !p.asp.map_new(p.brk_mapped, true, false) {
            return p.brk as i64;
        }
        p.brk_mapped += PAGE;
    }
    if want < p.brk {
        // Linux hands back zeroes when the heap regrows: scrub what was given up.
        let zero = [0u8; 4096];
        let mut a = want;
        while a < p.brk {
            let n = ((PAGE - (a & (PAGE - 1))).min(p.brk - a)) as usize;
            p.asp.copy_out(a, &zero[..n], true);
            a += n as u64;
        }
    }
    p.brk = want;
    want as i64
}

/// Local TLB flush after live PTE edits. The task is pinned to this core, so a reload of the current
/// CR3 is the whole shoot-down.
fn flush() {
    unsafe { super::memory::load_cr3(super::memory::current_cr3()) };
}

fn now_ts(p: &LinuxProc, buf: u64) -> i64 {
    let ms = crate::arch::ms();
    let mut t = [0u8; 16];
    t[0..8].copy_from_slice(&(ms / 1000).to_le_bytes());
    t[8..16].copy_from_slice(&((ms % 1000) * 1_000_000).to_le_bytes());
    if p.asp.copy_out(buf, &t, false) { 0 } else { -EFAULT }
}

pub fn handle(p: &mut LinuxProc, nr: u64, a: [u64; 6]) -> i64 {
    match nr {
        0 => {
            // read
            let (fd, buf, cnt) = (a[0], a[1], a[2]);
            let cnt = cnt.min(1 << 20) as usize;
            match fd_slot(p, fd) {
                None => -EBADF,
                Some(Fd::Console) => 0, // stdin: EOF in rung 1
                Some(Fd::File { dir: true, .. }) => -21,
                Some(Fd::File { data, pos, .. }) => {
                    let start = (*pos as usize).min(data.len());
                    let n = cnt.min(data.len() - start);
                    let chunk: Vec<u8> = data[start..start + n].to_vec();
                    *pos += n as u64;
                    if p.asp.copy_out(buf, &chunk, false) { n as i64 } else { -EFAULT }
                }
            }
        }
        1 => match fd_slot(p, a[0]) {
            Some(Fd::Console) => console_write(p, a[1], a[2]),
            Some(_) => -EBADF,
            None => -EBADF,
        },
        2 => do_open(p, AT_FDCWD, a[0], a[1]),
        257 => do_open(p, a[0] as i64, a[1], a[2]),
        3 => {
            if fd_slot(p, a[0]).is_none() {
                return -EBADF;
            }
            if a[0] >= 3 {
                p.fds[a[0] as usize] = None;
            }
            0
        }
        5 => fstat(p, a[0], a[1]),
        262 => {
            // newfstatat(dirfd, path, buf, flags)
            let Some(raw) = p.asp.read_cstr(a[1], 4096) else { return -EFAULT };
            if raw.is_empty() && a[3] & AT_EMPTY_PATH != 0 {
                return fstat(p, a[0], a[2]);
            }
            let Ok(path) = core::str::from_utf8(&raw) else { return -EINVAL };
            let mut full = String::from(path);
            if !full.starts_with('/') {
                let mut c = p.cwd.clone();
                if !c.ends_with('/') {
                    c.push('/');
                }
                c.push_str(&full);
                full = c;
            }
            use crate::fs::vfs::NodeKind;
            let vp = crate::shell::vfs_path(&full);
            match crate::shell::vfs_mount_table().stat(&vp) {
                Ok(st) => put_stat(p, a[2], if matches!(st.kind, NodeKind::Dir) { 0o040555 } else { 0o100444 }, st.size),
                Err(_) => -ENOENT,
            }
        }
        8 => {
            let (off, whence) = (a[1] as i64, a[2]);
            match fd_slot(p, a[0]) {
                None => -EBADF,
                Some(Fd::Console) => -ESPIPE,
                Some(Fd::File { data, pos, .. }) => {
                    let base = match whence {
                        0 => 0i64,
                        1 => *pos as i64,
                        2 => data.len() as i64,
                        _ => return -EINVAL,
                    };
                    match base.checked_add(off) {
                        Some(n) if n >= 0 => {
                            *pos = n as u64;
                            n
                        }
                        _ => -EINVAL,
                    }
                }
            }
        }
        9 => mmap(p, a[0], a[1], a[2], a[3], a[4], a[5]),
        10 => mprotect(p, a[0], a[1], a[2]),
        11 => munmap(p, a[0], a[1]),
        12 => brk(p, a[0]),
        13 | 14 => 0, // rt_sigaction / rt_sigprocmask: accepted, never delivered
        16 => {
            // ioctl: TCGETS on the std fds says "tty"; everything else is -ENOTTY
            if a[1] == 0x5401 && a[0] < 3 {
                if p.asp.copy_out(a[2], &[0u8; 36], false) { 0 } else { -EFAULT }
            } else {
                -ENOTTY
            }
        }
        20 => {
            // writev(fd, iov, cnt)
            if !matches!(fd_slot(p, a[0]), Some(Fd::Console)) {
                return -EBADF;
            }
            if a[2] > 1024 {
                return -EINVAL;
            }
            let mut total = 0i64;
            for i in 0..a[2] {
                let mut iov = [0u8; 16];
                if !p.asp.copy_in(a[1] + i * 16, &mut iov) {
                    return if total > 0 { total } else { -EFAULT };
                }
                let base = u64::from_le_bytes([iov[0], iov[1], iov[2], iov[3], iov[4], iov[5], iov[6], iov[7]]);
                let len = u64::from_le_bytes([iov[8], iov[9], iov[10], iov[11], iov[12], iov[13], iov[14], iov[15]]);
                let r = console_write(p, base, len);
                if r < 0 {
                    return if total > 0 { total } else { r };
                }
                total += r;
            }
            total
        }
        39 | 186 => crate::arch::sched::current_task_id(crate::arch::percpu::this_cpu().cpu_index as usize).unwrap_or(1) as i64,
        110 => 1,
        63 => {
            let mut u = [0u8; 390];
            for (i, s) in ["Linux", "unaos", "6.1.0-unaos", "#1 UnaOS linuxabi", "x86_64", "(none)"].iter().enumerate() {
                u[i * 65..i * 65 + s.len()].copy_from_slice(s.as_bytes());
            }
            if p.asp.copy_out(a[0], &u, false) { 0 } else { -EFAULT }
        }
        79 => {
            let mut c = Vec::from(p.cwd.as_bytes());
            c.push(0);
            if (c.len() as u64) > a[1] {
                return -34; // ERANGE
            }
            if p.asp.copy_out(a[0], &c, false) { c.len() as i64 } else { -EFAULT }
        }
        89 | 267 => -ENOENT,
        158 => match a[0] {
            0x1002 => {
                if x86_64::VirtAddr::try_new(a[1]).is_err() {
                    return -EINVAL;
                }
                p.fs_base = a[1];
                if let Ok(v) = x86_64::VirtAddr::try_new(a[1]) {
                    x86_64::registers::model_specific::FsBase::write(v);
                }
                0
            }
            0x1003 => {
                if p.asp.copy_out(a[1], &p.fs_base.to_le_bytes(), false) { 0 } else { -EFAULT }
            }
            _ => -EINVAL,
        },
        218 => crate::arch::sched::current_task_id(crate::arch::percpu::this_cpu().cpu_index as usize).unwrap_or(1) as i64,
        228 => now_ts(p, a[1]),
        102 | 104 | 107 | 108 => 0,
        _ => {
            let n = nr as u32;
            if !p.enosys.contains(&n) {
                p.enosys.push(n);
                serial_println!("[linuxabi] enosys nr={}", nr);
            }
            -ENOSYS
        }
    }
}
