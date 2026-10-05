// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! LINUXABI M2 + LINUXABI2 — the Linux x86_64 syscall table.
//!
//! Every user pointer goes through `AddrSpace::copy_in`/`copy_out` (software page-table walk, no kernel deref of a user VA).
//! [`handle`] is a NON-BLOCKING try-op: `RETRY` means "would block, nothing was consumed" and `dispatch` re-runs it after a yield.
//! Files are read through the VFS per call (SELFBUILD3; they were slurped whole at `open`); writes go THROUGH the mount table, only under the session's `/home/<user>/`.
//! Unknown numbers answer `-ENOSYS` with a once-per-number `[linuxabi] enosys nr=` line.

use super::fd::{self, Desc, FdEnt, Kind, O_APPEND, PIPE_CAP};
use super::proc::{self, ProcInfo};
use super::LinuxProc; // SELFBUILD3: the memory syscalls (and their constants) moved to vm.rs
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

/// `handle` result meaning "would block — nothing consumed, call me again after a yield".
pub const RETRY: i64 = i64::MIN;
const ENOENT: i64 = 2;
const EIO: i64 = 5;
const EBADF: i64 = 9;
const EAGAIN: i64 = 11;
const EACCES: i64 = 13;
const EFAULT: i64 = 14;
const EEXIST: i64 = 17;
const ENOTDIR: i64 = 20;
const EISDIR: i64 = 21;
const EINVAL: i64 = 22;
const EMFILE: i64 = 24;
const ENOTTY: i64 = 25;
const ESPIPE: i64 = 29;
const EPIPE: i64 = 32;
const ENOSYS: i64 = 38;
const ENOTEMPTY: i64 = 39;
pub const AT_FDCWD: i64 = -100;
const AT_EMPTY_PATH: u64 = 0x1000;
const AT_REMOVEDIR: u64 = 0x200;
const O_CREAT: u64 = 0o100;
const O_EXCL: u64 = 0o200;
const O_TRUNC: u64 = 0o1000;
const O_DIRECTORY: u64 = 0o200000;
const O_CLOEXEC: u64 = 0o2000000;
const MAX_FD: usize = 256;

pub fn get(p: &LinuxProc, fd: u64) -> Option<Arc<Desc>> {
    p.fds.get(fd as usize).and_then(|s| s.as_ref()).map(|e| e.d.clone())
}

pub fn install(p: &mut LinuxProc, d: Arc<Desc>, min: usize, cloexec: bool) -> i64 {
    let mut i = min;
    while i < MAX_FD {
        if i >= p.fds.len() {
            p.fds.resize(i + 1, None);
        }
        if p.fds[i].is_none() {
            p.fds[i] = Some(FdEnt { d, cloexec });
            return i as i64;
        }
        i += 1;
    }
    -EMFILE
}

// ---- namespace helpers (DIRNS: the mount table) ----

pub fn home_prefix() -> String {
    #[cfg(feature = "login")]
    {
        let mut b = [0u8; crate::fs::users::NAME_MAX];
        if let Some(n) = crate::fs::users::whoami(&mut b) {
            if let Ok(name) = core::str::from_utf8(&b[..n]) {
                return alloc::format!("/home/{}/", name);
            }
        }
    }
    String::from("/home/")
}

/// The DIRNS write rule (the editor's): only under the session user's `/home/<user>/`, never through `..`.
pub fn may_write(path: &str) -> bool {
    if path.split('/').any(|c| c == "..") {
        return false;
    }
    let pre = home_prefix();
    path.len() > pre.len() && path.as_bytes()[..pre.len()].eq_ignore_ascii_case(pre.as_bytes())
}

fn join(base: &str, rel: &str) -> String {
    if rel.starts_with('/') {
        return String::from(rel);
    }
    let mut c = String::from(base);
    if !c.ends_with('/') {
        c.push('/');
    }
    c.push_str(rel);
    c
}

/// Resolve a user path string (NUL-terminated at `va`) against cwd / a directory fd, lexically normalised.
pub fn resolve(p: &LinuxProc, dirfd: i64, va: u64) -> Result<String, i64> {
    let Some(raw) = p.asp.read_cstr(va, 4096) else { return Err(-EFAULT) };
    let Ok(path) = core::str::from_utf8(&raw) else { return Err(-EINVAL) };
    if path.is_empty() {
        return Err(-ENOENT);
    }
    let base = if path.starts_with('/') || dirfd == AT_FDCWD {
        p.cwd.clone()
    } else {
        match get(p, dirfd as u64) {
            Some(d) => match &*fd::lk(&d.k) {
                Kind::Dir { path, .. } => path.clone(),
                _ => return Err(-ENOTDIR),
            },
            None => return Err(-EBADF),
        }
    };
    Ok(crate::shell::vfs_path(&join(&base, path)))
}

fn mount_ancestor(mt: &crate::fs::vfs::MountTable, path: &str) -> bool {
    let pre = if path.ends_with('/') { String::from(path) } else { alloc::format!("{}/", path) };
    mt.prefixes().iter().any(|x| x.starts_with(pre.as_str()))
}

/// `(is_dir, size)`.
pub fn stat_full(path: &str) -> Result<(bool, u64), i64> {
    use crate::fs::vfs::NodeKind;
    let mt = crate::shell::vfs_mount_table();
    match mt.stat(path) {
        Ok(s) => Ok((matches!(s.kind, NodeKind::Dir), s.size)),
        Err(_) if path == "/" || mount_ancestor(&mt, path) => Ok((true, 0)),
        Err(_) => Err(-ENOENT),
    }
}

/// A directory listing: `.`, `..`, the backend's entries, and the first components of deeper mount prefixes.
fn list_dir(path: &str) -> Result<Vec<(String, bool)>, i64> {
    use crate::fs::vfs::NodeKind;
    let mt = crate::shell::vfs_mount_table();
    let mut v: Vec<(String, bool)> = Vec::new();
    v.push((String::from("."), true));
    v.push((String::from(".."), true));
    match mt.read_dir(path) {
        Ok(es) => {
            for e in es {
                v.push((e.name, matches!(e.kind, NodeKind::Dir)));
            }
        }
        Err(_) if path == "/" || mount_ancestor(&mt, path) => {}
        Err(_) => return Err(-ENOENT),
    }
    let pre = if path.ends_with('/') { String::from(path) } else { alloc::format!("{}/", path) };
    for x in mt.prefixes() {
        if x.len() > pre.len() && x.starts_with(pre.as_str()) {
            let comp = x[pre.len()..].split('/').next().unwrap_or("");
            if !comp.is_empty() && !v.iter().any(|(n, _)| n.eq_ignore_ascii_case(comp)) {
                v.push((String::from(comp), true));
            }
        }
    }
    Ok(v)
}

/// SELFBUILD3: a file's CURRENT size as the VFS reports it (every description sees the same file; 0 if it vanished).
pub fn fsize(path: &str) -> u64 {
    crate::shell::vfs_mount_table().stat(path).map(|s| s.size).unwrap_or(0)
}

/// SELFBUILD3: `cnt` bytes of `path` at `off` straight from the VFS (short at EOF; `Err(-EIO)` on a backend failure).
pub fn file_read(path: &str, off: u64, cnt: usize) -> Result<Vec<u8>, i64> {
    let size = fsize(path);
    if off >= size || cnt == 0 {
        return Ok(Vec::new());
    }
    let n = (cnt as u64).min(size - off) as usize;
    crate::shell::vfs_mount_table().read(path, off, n).map_err(|_| -EIO)
}

fn do_open(p: &mut LinuxProc, dirfd: i64, path_va: u64, flags: u64, _mode: u64) -> i64 {
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL};
    let full = match resolve(p, dirfd, path_va) {
        Ok(f) => f,
        Err(e) => return e,
    };
    let acc = flags & 3;
    let want_write = acc != 0 || flags & O_TRUNC != 0;
    let fl = (flags as u32) & (fd::O_NONBLOCK | O_APPEND);
    let cloexec = flags & O_CLOEXEC != 0;
    match stat_full(&full) {
        Ok((true, _)) => {
            if want_write {
                return -EISDIR;
            }
            let ents = match list_dir(&full) {
                Ok(e) => e,
                Err(e) => return e,
            };
            install(p, Desc::new(Kind::Dir { path: full, ents, pos: 0 }, fl), 0, cloexec)
        }
        Ok((false, _)) => {
            if flags & O_CREAT != 0 && flags & O_EXCL != 0 {
                return -EEXIST;
            }
            if flags & O_DIRECTORY != 0 {
                return -ENOTDIR;
            }
            if want_write && !may_write(&full) {
                return -EACCES;
            }
            if flags & O_TRUNC != 0 && want_write {
                let mt = crate::shell::vfs_mount_table();
                let _ = mt.unlink(&full, KERNEL_PRINCIPAL);
                if mt.create(&full, NodeKind::File, KERNEL_PRINCIPAL).is_err() {
                    return -EACCES;
                }
            } // SELFBUILD3: no slurp — read/pread/mmap go to the VFS (and the block cache under it) per call
            install(p, Desc::new(Kind::File { path: full, pos: 0, read: acc != 1, write: acc != 0 }, fl), 0, cloexec)
        }
        Err(e) => {
            if flags & O_CREAT == 0 {
                return e;
            }
            if !may_write(&full) {
                return -EACCES;
            }
            if crate::shell::vfs_mount_table().create(&full, NodeKind::File, KERNEL_PRINCIPAL).is_err() {
                return -EACCES;
            }
            install(p, Desc::new(Kind::File { path: full, pos: 0, read: acc != 1, write: acc != 0 }, fl), 0, cloexec)
        }
    }
}

fn put_stat(p: &LinuxProc, buf: u64, mode: u32, size: u64, ino: u64) -> i64 {
    let mut s = [0u8; 144];
    s[8..16].copy_from_slice(&ino.to_le_bytes());
    s[16..24].copy_from_slice(&1u64.to_le_bytes()); // st_nlink
    s[24..28].copy_from_slice(&mode.to_le_bytes()); // st_uid / st_gid stay 0 = root
    s[48..56].copy_from_slice(&size.to_le_bytes());
    s[56..64].copy_from_slice(&4096u64.to_le_bytes()); // st_blksize
    s[64..72].copy_from_slice(&size.div_ceil(512).to_le_bytes()); // st_blocks
    if p.asp.copy_out(buf, &s, false) { 0 } else { -EFAULT }
}

pub fn ino_of(path: &str) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in path.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    (h >> 1) | 1
}

fn stat_path(p: &LinuxProc, dirfd: i64, path_va: u64, buf: u64) -> i64 {
    let full = match resolve(p, dirfd, path_va) {
        Ok(f) => f,
        Err(e) => return e,
    };
    match stat_full(&full) {
        Ok((dir, size)) => put_stat(p, buf, if dir { 0o040755 } else { 0o100644 }, size, ino_of(&full)),
        Err(e) => e,
    }
}

fn fstat(p: &LinuxProc, fd: u64, buf: u64) -> i64 {
    let Some(d) = get(p, fd) else { return -EBADF };
    let (mode, size, ino) = match &*fd::lk(&d.k) {
        Kind::Console => (0o020620u32, 0u64, 1),
        Kind::File { path, .. } => (0o100644, fsize(path), ino_of(path)),
        Kind::Dir { path, .. } => (0o040755, 0, ino_of(path)),
        Kind::PipeR(pp) | Kind::PipeW(pp) => (0o010600, fd::lk(&pp.buf).len() as u64, 2),
        Kind::Ext(x) => (super::sys3::mode(x), 0, 3), // SELFBUILD2
    };
    put_stat(p, buf, mode, size, ino)
}

fn console_out(v: &[u8]) {
    serial_print!("{}", String::from_utf8_lossy(v));
    fd::out_push(v);
}

pub fn do_read(p: &mut LinuxProc, fd: u64, buf: u64, cnt: u64, off: Option<u64>) -> i64 {
    let Some(d) = get(p, fd) else { return -EBADF };
    let cnt = cnt.min(1 << 20) as usize;
    if cnt == 0 {
        return 0;
    }
    if !p.asp.writable_range(buf, cnt) {
        return -EFAULT; // validated BEFORE a pipe/stdin read consumes anything
    }
    let nb = d.nonblock();
    let data: Vec<u8> = {
        let mut k = fd::lk(&d.k);
        match &mut *k {
            Kind::Console => {
                let v = fd::stdin_take_line(cnt);
                if v.is_empty() {
                    if fd::stdin_eof_take() {
                        return 0; // LINUXABI3 M5: Ctrl-D in the `linux` verb
                    }
                    return if nb { -EAGAIN } else { RETRY };
                }
                v
            }
            Kind::File { path, pos, read, .. } => {
                if !*read {
                    return -EBADF;
                }
                let v = match file_read(path, off.unwrap_or(*pos), cnt) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                if off.is_none() {
                    *pos += v.len() as u64;
                }
                v
            }
            Kind::Dir { .. } => return -EISDIR,
            Kind::PipeR(pp) => {
                let mut b = fd::lk(&pp.buf);
                let n = cnt.min(b.len());
                if n == 0 {
                    if pp.writers.load(Ordering::Acquire) == 0 {
                        return 0; // EOF: the last writer closed
                    }
                    return if nb { -EAGAIN } else { RETRY };
                }
                b.drain(..n).collect()
            }
            Kind::PipeW(_) => return -EBADF,
            Kind::Ext(x) => match super::sys3::read_ext(x, cnt, nb) {
                Ok(v) => v, // SELFBUILD2: eventfd / socket
                Err(e) => return e,
            },
        }
    };
    p.asp.copy_out(buf, &data, false);
    data.len() as i64
}

pub fn do_write(p: &mut LinuxProc, fd: u64, buf: u64, cnt: u64, off: Option<u64>) -> i64 {
    let Some(d) = get(p, fd) else { return -EBADF };
    let cnt = cnt.min(1 << 20) as usize;
    if cnt == 0 {
        return 0;
    }
    let mut v = alloc::vec![0u8; cnt];
    if !p.asp.copy_in(buf, &mut v) {
        return -EFAULT;
    }
    write_desc(&d, &v, off)
}

/// SELFBUILD2: the body of `write` on a description, from kernel bytes (`sendfile` shares it).
pub fn write_desc(d: &Desc, v: &[u8], off: Option<u64>) -> i64 {
    use crate::fs::vfs::KERNEL_PRINCIPAL;
    let cnt = v.len();
    let nb = d.nonblock();
    let append = d.flags.load(Ordering::Relaxed) & O_APPEND != 0;
    let mut k = fd::lk(&d.k);
    match &mut *k {
        Kind::Console => {
            console_out(&v);
            cnt as i64
        }
        Kind::File { path, pos, write, .. } => {
            if !*write {
                return -EBADF;
            }
            let at = if append { fsize(path) as usize } else { off.unwrap_or(*pos) as usize };
            let mt = crate::shell::vfs_mount_table();
            let mut done = 0usize;
            while done < v.len() {
                let n = (v.len() - done).min(4096);
                match mt.write(path, (at + done) as u64, &v[done..done + n], KERNEL_PRINCIPAL) {
                    Ok(w) if w > 0 => done += w,
                    _ => break,
                }
            }
            if done == 0 {
                return -EIO;
            }
            let end = at + done;
            if off.is_none() {
                *pos = end as u64;
            }
            done as i64
        }
        Kind::PipeW(pp) => {
            if pp.readers.load(Ordering::Acquire) == 0 {
                return -EPIPE;
            }
            let mut b = fd::lk(&pp.buf);
            let space = PIPE_CAP - b.len();
            if space == 0 {
                return if nb { -EAGAIN } else { RETRY };
            }
            let n = space.min(cnt);
            b.extend(v[..n].iter().copied());
            n as i64
        }
        Kind::Ext(x) => super::sys3::write_ext(x, v, nb), // SELFBUILD2
        _ => -EBADF,
    }
}

fn iov_io(p: &mut LinuxProc, fd: u64, iov: u64, cnt: u64, write: bool) -> i64 {
    if cnt > 1024 {
        return -EINVAL;
    }
    let mut total = 0i64;
    for i in 0..cnt {
        let mut e = [0u8; 16];
        if !p.asp.copy_in(iov + i * 16, &mut e) {
            return if total > 0 { total } else { -EFAULT };
        }
        let base = u64::from_le_bytes([e[0], e[1], e[2], e[3], e[4], e[5], e[6], e[7]]);
        let len = u64::from_le_bytes([e[8], e[9], e[10], e[11], e[12], e[13], e[14], e[15]]);
        if len == 0 {
            continue;
        }
        let r = if write { do_write(p, fd, base, len, None) } else { do_read(p, fd, base, len, None) };
        if r == RETRY && total > 0 {
            return total;
        }
        if r < 0 {
            return if total > 0 { total } else { r };
        }
        total += r;
        if (r as u64) < len {
            break;
        }
    }
    total
}

fn getdents(p: &mut LinuxProc, fd: u64, buf: u64, cnt: u64) -> i64 {
    let Some(d) = get(p, fd) else { return -EBADF };
    let cnt = cnt.min(65536) as usize;
    let mut out: Vec<u8> = Vec::new();
    {
        let mut k = fd::lk(&d.k);
        let Kind::Dir { path, ents, pos } = &mut *k else { return -ENOTDIR };
        while *pos < ents.len() {
            let (name, is_dir) = &ents[*pos];
            let reclen = (19 + name.len() + 1 + 7) & !7;
            if out.len() + reclen > cnt {
                break;
            }
            let ino = ino_of(&join(path, name));
            let at = out.len();
            out.resize(at + reclen, 0);
            out[at..at + 8].copy_from_slice(&ino.to_le_bytes());
            out[at + 8..at + 16].copy_from_slice(&((*pos + 1) as i64).to_le_bytes());
            out[at + 16..at + 18].copy_from_slice(&(reclen as u16).to_le_bytes());
            out[at + 18] = if *is_dir { 4 } else { 8 }; // DT_DIR / DT_REG (from the FAT directory attribute)
            out[at + 19..at + 19 + name.len()].copy_from_slice(name.as_bytes());
            *pos += 1;
        }
        if out.is_empty() && *pos < ents.len() {
            return -EINVAL; // buffer too small for even one entry
        }
    }
    if !out.is_empty() && !p.asp.copy_out(buf, &out, false) {
        return -EFAULT;
    }
    out.len() as i64
}

fn do_lseek(p: &mut LinuxProc, fd: u64, off: i64, whence: u64) -> i64 {
    let Some(d) = get(p, fd) else { return -EBADF };
    let mut k = fd::lk(&d.k);
    match &mut *k {
        Kind::Console | Kind::PipeR(_) | Kind::PipeW(_) | Kind::Ext(_) => -ESPIPE,
        Kind::Dir { pos, .. } => {
            if off == 0 && whence == 0 {
                *pos = 0;
                0
            } else {
                -EINVAL
            }
        }
        Kind::File { path, pos, .. } => {
            let base = match whence {
                0 => 0i64,
                1 => *pos as i64,
                2 => fsize(path) as i64,
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

fn do_pipe(p: &mut LinuxProc, buf: u64, flags: u64) -> i64 {
    if !p.asp.writable_range(buf, 8) {
        return -EFAULT;
    }
    let (r, w) = fd::new_pipe((flags as u32) & fd::O_NONBLOCK);
    let cx = flags & O_CLOEXEC != 0;
    let rf = install(p, r, 0, cx);
    if rf < 0 {
        return rf;
    }
    let wf = install(p, w, 0, cx);
    if wf < 0 {
        p.fds[rf as usize] = None;
        return wf;
    }
    let mut o = [0u8; 8];
    o[0..4].copy_from_slice(&(rf as i32).to_le_bytes());
    o[4..8].copy_from_slice(&(wf as i32).to_le_bytes());
    p.asp.copy_out(buf, &o, false);
    0
}

fn do_dup(p: &mut LinuxProc, old: u64, new: Option<u64>, cloexec: bool) -> i64 {
    let Some(d) = get(p, old) else { return -EBADF };
    match new {
        None => install(p, d, 0, false),
        Some(n) => {
            if n as usize >= MAX_FD {
                return -EBADF;
            }
            if n as usize >= p.fds.len() {
                p.fds.resize(n as usize + 1, None);
            }
            p.fds[n as usize] = Some(FdEnt { d, cloexec });
            n as i64
        }
    }
}

fn do_fcntl(p: &mut LinuxProc, fd: u64, cmd: u64, arg: u64) -> i64 {
    let Some(d) = get(p, fd) else { return -EBADF };
    match cmd {
        0 | 1030 => install(p, d, arg as usize, cmd == 1030), // F_DUPFD / F_DUPFD_CLOEXEC
        1 => p.fds[fd as usize].as_ref().map_or(-EBADF, |e| e.cloexec as i64), // F_GETFD
        2 => {
            if let Some(Some(e)) = p.fds.get_mut(fd as usize) {
                e.cloexec = arg & 1 != 0;
            }
            0
        }
        3 => {
            let f = d.flags.load(Ordering::Relaxed) as i64;
            let acc = match &*fd::lk(&d.k) {
                Kind::File { read: true, write: true, .. } => 2,
                Kind::File { write: true, .. } | Kind::PipeW(_) => 1,
                _ => 0,
            };
            f | acc
        }
        4 => {
            let keep = d.flags.load(Ordering::Relaxed) & !(fd::O_NONBLOCK | O_APPEND);
            d.flags.store(keep | ((arg as u32) & (fd::O_NONBLOCK | O_APPEND)), Ordering::Relaxed);
            0
        }
        _ => -EINVAL,
    }
}

/// poll revents for one description.
pub fn poll_ready(d: &Desc, events: i16) -> i16 {
    const IN: i16 = 1;
    const OUT: i16 = 4;
    const ERR: i16 = 8;
    const HUP: i16 = 16;
    let mut r = 0i16;
    match &*fd::lk(&d.k) {
        Kind::Console => {
            if fd::stdin_has_data() || fd::stdin_eof_pending() {
                r |= IN;
            }
            r |= OUT;
        }
        Kind::File { .. } | Kind::Dir { .. } => r |= IN | OUT,
        Kind::PipeR(pp) => {
            if !fd::lk(&pp.buf).is_empty() {
                r |= IN;
            } else if pp.writers.load(Ordering::Acquire) == 0 {
                r |= HUP;
            }
        }
        Kind::PipeW(pp) => {
            if pp.readers.load(Ordering::Acquire) == 0 {
                r |= ERR;
            } else if fd::lk(&pp.buf).len() < PIPE_CAP {
                r |= OUT;
            }
        }
        Kind::Ext(x) => r |= super::sys3::ready(x), // SELFBUILD2
    }
    r & (events | ERR | HUP)
}

/// `timeout_ms`: <0 = forever, 0 = poll once.
fn do_poll(p: &mut LinuxProc, fds_va: u64, n: u64, timeout_ms: i64) -> i64 {
    if n > 256 {
        return -EINVAL;
    }
    let mut ready = 0i64;
    let mut recs: Vec<[u8; 8]> = Vec::new();
    for i in 0..n {
        let mut e = [0u8; 8];
        if !p.asp.copy_in(fds_va + i * 8, &mut e) {
            return -EFAULT;
        }
        let fd = i32::from_le_bytes([e[0], e[1], e[2], e[3]]);
        let ev = i16::from_le_bytes([e[4], e[5]]);
        let rev: i16 = if fd < 0 { 0 } else { match get(p, fd as u64) { Some(d) => poll_ready(&d, ev), None => 32 /* POLLNVAL */ } };
        e[6..8].copy_from_slice(&rev.to_le_bytes());
        if rev != 0 {
            ready += 1;
        }
        recs.push(e);
    }
    let now = crate::arch::ms();
    if ready > 0 || timeout_ms == 0 || p.sleep_until.is_some_and(|t| now >= t) {
        p.sleep_until = None;
        for (i, e) in recs.iter().enumerate() {
            if !p.asp.copy_out(fds_va + i as u64 * 8, e, false) {
                return -EFAULT;
            }
        }
        return ready;
    }
    if timeout_ms > 0 && p.sleep_until.is_none() {
        p.sleep_until = Some(now + timeout_ms as u64);
    }
    RETRY
}

fn read_timespec_ms(p: &LinuxProc, va: u64) -> Result<u64, i64> {
    let mut t = [0u8; 16];
    if !p.asp.copy_in(va, &mut t) {
        return Err(-EFAULT);
    }
    let s = i64::from_le_bytes(t[0..8].try_into().unwrap());
    let ns = i64::from_le_bytes(t[8..16].try_into().unwrap());
    if s < 0 || !(0..1_000_000_000).contains(&ns) {
        return Err(-EINVAL);
    }
    Ok((s as u64).min(3600) * 1000 + ns as u64 / 1_000_000)
}

fn do_sleep(p: &mut LinuxProc, req_va: u64) -> i64 {
    let now = crate::arch::ms();
    match p.sleep_until {
        None => match read_timespec_ms(p, req_va) {
            Ok(ms) => {
                p.sleep_until = Some(now + ms);
                RETRY
            }
            Err(e) => e,
        },
        Some(t) if now >= t => {
            p.sleep_until = None;
            0
        }
        Some(_) => RETRY,
    }
}

fn do_access(p: &LinuxProc, dirfd: i64, path_va: u64, mode: u64) -> i64 {
    let full = match resolve(p, dirfd, path_va) {
        Ok(f) => f,
        Err(e) => return e,
    };
    if let Err(e) = stat_full(&full) {
        return e;
    }
    if mode & 2 != 0 && !may_write(&full) {
        return -EACCES;
    }
    0
}

fn do_mkdir(p: &LinuxProc, dirfd: i64, path_va: u64) -> i64 {
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL};
    let full = match resolve(p, dirfd, path_va) {
        Ok(f) => f,
        Err(e) => return e,
    };
    if !may_write(&full) {
        return -EACCES;
    }
    if stat_full(&full).is_ok() {
        return -EEXIST;
    }
    match crate::shell::vfs_mount_table().create(&full, NodeKind::Dir, KERNEL_PRINCIPAL) {
        Ok(_) => 0,
        Err(_) => -ENOENT,
    }
}

fn do_unlink(p: &LinuxProc, dirfd: i64, path_va: u64, rmdir: bool) -> i64 {
    use crate::fs::vfs::KERNEL_PRINCIPAL;
    let full = match resolve(p, dirfd, path_va) {
        Ok(f) => f,
        Err(e) => return e,
    };
    if !may_write(&full) {
        return -EACCES;
    }
    let (dir, _) = match stat_full(&full) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let mt = crate::shell::vfs_mount_table();
    if dir != rmdir {
        return if dir { -EISDIR } else { -ENOTDIR };
    }
    let r = if rmdir { mt.remove_dir(&full, KERNEL_PRINCIPAL) } else { mt.unlink(&full, KERNEL_PRINCIPAL) };
    match r {
        Ok(()) => 0,
        Err(_) if rmdir => -ENOTEMPTY,
        Err(_) => -EACCES,
    }
}

fn do_rename(p: &LinuxProc, from_va: u64, to_va: u64) -> i64 {
    use crate::fs::vfs::KERNEL_PRINCIPAL;
    let (f, t) = match (resolve(p, AT_FDCWD, from_va), resolve(p, AT_FDCWD, to_va)) {
        (Ok(f), Ok(t)) => (f, t),
        (Err(e), _) | (_, Err(e)) => return e,
    };
    if !may_write(&f) || !may_write(&t) {
        return -EACCES;
    }
    if let Err(e) = stat_full(&f) {
        return e;
    }
    match crate::shell::vfs_mount_table().rename(&f, &t, KERNEL_PRINCIPAL) {
        Ok(()) => 0,
        Err(_) => -EACCES,
    }
}

fn rlimit(res: u64) -> [u8; 16] {
    let (cur, max) = match res {
        3 => (8u64 << 20, u64::MAX),
        7 => (MAX_FD as u64, MAX_FD as u64),
        _ => (u64::MAX, u64::MAX),
    };
    let mut o = [0u8; 16];
    o[0..8].copy_from_slice(&cur.to_le_bytes());
    o[8..16].copy_from_slice(&max.to_le_bytes());
    o
}

fn now_ts(p: &LinuxProc, buf: u64) -> i64 {
    let ms = crate::arch::ms();
    let mut t = [0u8; 16];
    t[0..8].copy_from_slice(&(ms / 1000).to_le_bytes());
    t[8..16].copy_from_slice(&((ms % 1000) * 1_000_000).to_le_bytes());
    if p.asp.copy_out(buf, &t, false) { 0 } else { -EFAULT }
}

pub fn handle(p: &mut LinuxProc, info: &Arc<ProcInfo>, ktop: u64, nr: u64, a: [u64; 6]) -> i64 {
    match nr {
        0 => do_read(p, a[0], a[1], a[2], None),
        1 => do_write(p, a[0], a[1], a[2], None),
        17 => do_read(p, a[0], a[1], a[2], Some(a[3])),
        18 => do_write(p, a[0], a[1], a[2], Some(a[3])),
        19 => iov_io(p, a[0], a[1], a[2], false),
        20 => iov_io(p, a[0], a[1], a[2], true),
        2 => do_open(p, AT_FDCWD, a[0], a[1], a[2]),
        257 => do_open(p, a[0] as i64, a[1], a[2], a[3]),
        3 => {
            if get(p, a[0]).is_none() {
                return -EBADF;
            }
            p.fds[a[0] as usize] = None; // dropping the last Arc<Desc> of a pipe end is the EOF/EPIPE edge
            0
        }
        4 | 6 => stat_path(p, AT_FDCWD, a[0], a[1]),
        5 => fstat(p, a[0], a[1]),
        262 => {
            // newfstatat(dirfd, path, buf, flags)
            if a[3] & AT_EMPTY_PATH != 0 && p.asp.read_cstr(a[1], 2).is_some_and(|s| s.is_empty()) {
                return fstat(p, a[0], a[2]);
            }
            stat_path(p, a[0] as i64, a[1], a[2])
        }
        7 => do_poll(p, a[0], a[1], a[2] as i32 as i64),
        271 => {
            // ppoll(fds, n, timespec*, sigmask, sz)
            let t = if a[2] == 0 {
                -1
            } else {
                match read_timespec_ms(p, a[2]) {
                    Ok(ms) => ms as i64,
                    Err(e) => return e,
                }
            };
            do_poll(p, a[0], a[1], t)
        }
        8 => do_lseek(p, a[0], a[1] as i64, a[2]),
        9 => super::vm::mmap(p, a[0], a[1], a[2], a[3], a[4], a[5]), // SELFBUILD3: lazy VMAs (vm.rs)
        10 => super::vm::mprotect(p, a[0], a[1], a[2]),
        11 => super::vm::munmap(p, a[0], a[1]),
        12 => super::vm::brk(p, a[0]),
        26 => super::vm::msync(p, a[0], a[1], a[2]),
        27 => super::vm::mincore(p, a[0], a[1], a[2]),
        13 => super::signal::sigaction(p, info, a), // SELFBUILD2: a per-process handler table (SELFBUILD1 accepted, never delivered)
        14 => super::signal::sigprocmask(p, info, a), // SELFBUILD2: a real blocked mask
        16 => {
            // ioctl: TCGETS / TIOCGWINSZ on the console say "tty"; everything else is -ENOTTY
            let is_con = get(p, a[0]).is_some_and(|d| matches!(&*fd::lk(&d.k), Kind::Console));
            if is_con && a[1] == 0x5401 {
                if p.asp.copy_out(a[2], &[0u8; 36], false) { 0 } else { -EFAULT }
            } else if a[1] == 0x5413 && a[0] < 3 {
                // TERMCOLOR M3 — TIOCGWINSZ: struct winsize { u16 rows, cols, xpixel, ypixel } from the console's geometry
                let (c, r) = crate::console::geometry();
                let mut ws = [0u8; 8];
                ws[0..2].copy_from_slice(&(r.min(65535) as u16).to_le_bytes());
                ws[2..4].copy_from_slice(&(c.min(65535) as u16).to_le_bytes());
                if p.asp.copy_out(a[2], &ws, false) { 0 } else { -EFAULT }
            } else if is_con && a[1] == 0x5413 {
                let mut w = [0u8; 8];
                w[0..2].copy_from_slice(&25u16.to_le_bytes());
                w[2..4].copy_from_slice(&80u16.to_le_bytes());
                if p.asp.copy_out(a[2], &w, false) { 0 } else { -EFAULT }
            } else {
                -ENOTTY
            }
        }
        21 => do_access(p, AT_FDCWD, a[0], a[1]),
        269 => do_access(p, a[0] as i64, a[1], a[2]),
        22 => do_pipe(p, a[0], 0),
        293 => do_pipe(p, a[0], a[1]),
        24 => 0, // sched_yield
        28 => super::vm::madvise(p, a[0], a[1], a[2]), // SELFBUILD3: DONTNEED/FREE drop pages
        32 => do_dup(p, a[0], None, false),
        33 => {
            if a[0] == a[1] {
                return if get(p, a[0]).is_some() { a[1] as i64 } else { -EBADF };
            }
            do_dup(p, a[0], Some(a[1]), false)
        }
        292 => {
            if a[0] == a[1] {
                return -EINVAL;
            }
            do_dup(p, a[0], Some(a[1]), a[2] & O_CLOEXEC != 0)
        }
        35 => do_sleep(p, a[0]),
        230 => do_sleep(p, a[2]),
        39 => info.pid as i64,
        186 => super::thread::gettid(info), // SELFBUILD2: tid != tgid in a threaded process
        218 => super::thread::set_tid_address(info, a[0]),
        15 => super::signal::sigreturn(p, info, ktop), // SELFBUILD2 M3
        130 => super::signal::sigsuspend(p, info, a),
        34 => RETRY, // pause: until a caught signal ends it (-EINTR from the dispatch loop)
        202 => super::thread::futex(p, info, a), // SELFBUILD2 M1
        435 => super::thread::clone3(p, info, ktop, a),
        110 => info.ppid.load(Ordering::Acquire) as i64,
        56 => proc::clone(p, info, ktop, a),
        57 | 58 => proc::fork(p, info, ktop, 0),
        59 => proc::execve(p, info, ktop, a[0], a[1], a[2]),
        61 => proc::wait4(p, info, a[0] as i32 as i64, a[1], a[2]),
        247 => proc::waitid(p, info, a[0], a[1], a[2], a[3]),
        62 => proc::kill(info, a[0] as i32 as i64, a[1]),
        200 | 234 => proc::kill(info, a[if nr == 234 { 1 } else { 0 }] as i32 as i64, a[if nr == 234 { 2 } else { 1 }]),
        109 => proc::setpgid(info, a[0], a[1]),
        111 => info.pgid.load(Ordering::Acquire) as i64,
        121 => proc::getpgid(info, a[0]),
        112 => {
            info.pgid.store(info.pid, Ordering::Release);
            info.pid as i64
        }
        124 => info.pgid.load(Ordering::Acquire) as i64,
        63 => {
            let mut u = [0u8; 390];
            for (i, s) in ["Linux", "unaos", "6.1.0-unaos", "#1 UnaOS linuxabi", "x86_64", "(none)"].iter().enumerate() {
                u[i * 65..i * 65 + s.len()].copy_from_slice(s.as_bytes());
            }
            if p.asp.copy_out(a[0], &u, false) { 0 } else { -EFAULT }
        }
        72 => do_fcntl(p, a[0], a[1], a[2]),
        79 => {
            let mut c = Vec::from(p.cwd.as_bytes());
            c.push(0);
            if (c.len() as u64) > a[1] {
                return -34; // ERANGE
            }
            if p.asp.copy_out(a[0], &c, false) { c.len() as i64 } else { -EFAULT }
        }
        80 => {
            let full = match resolve(p, AT_FDCWD, a[0]) {
                Ok(f) => f,
                Err(e) => return e,
            };
            match stat_full(&full) {
                Ok((true, _)) => {
                    p.cwd = full;
                    0
                }
                Ok(_) => -ENOTDIR,
                Err(e) => e,
            }
        }
        82 => do_rename(p, a[0], a[1]),
        83 => do_mkdir(p, AT_FDCWD, a[0]),
        258 => do_mkdir(p, a[0] as i64, a[1]),
        84 => do_unlink(p, AT_FDCWD, a[0], true),
        87 => do_unlink(p, AT_FDCWD, a[0], false),
        263 => do_unlink(p, a[0] as i64, a[1], a[2] & AT_REMOVEDIR != 0),
        89 | 267 => super::sys2::readlink(p, nr, a), // readlink: no symlinks exist (EINVAL = "not a symlink"); SELFBUILD1: /proc/self/exe
        95 => {
            let old = p.umask;
            p.umask = (a[0] & 0o777) as u32;
            old as i64
        }
        96 => {
            let ms = crate::arch::ms();
            let mut t = [0u8; 16];
            t[0..8].copy_from_slice(&(ms / 1000).to_le_bytes());
            t[8..16].copy_from_slice(&((ms % 1000) * 1000).to_le_bytes());
            if a[0] == 0 || p.asp.copy_out(a[0], &t, false) { 0 } else { -EFAULT }
        }
        97 => {
            if p.asp.copy_out(a[1], &rlimit(a[0]), false) { 0 } else { -EFAULT }
        }
        302 => {
            if a[3] != 0 && !p.asp.copy_out(a[3], &rlimit(a[1]), false) {
                return -EFAULT;
            }
            0
        }
        99 => {
            let mut s = [0u8; 112];
            s[0..8].copy_from_slice(&((crate::arch::ms() / 1000) as i64).to_le_bytes());
            let (total, free) = super::vm::sysinfo_bytes(p.asp.pages()); // SELFBUILD3: the resident budget
            s[32..40].copy_from_slice(&total.to_le_bytes());
            s[40..48].copy_from_slice(&free.to_le_bytes());
            s[80..82].copy_from_slice(&(proc::live_count() as u16).to_le_bytes());
            s[104..108].copy_from_slice(&1u32.to_le_bytes());
            if p.asp.copy_out(a[0], &s, false) { 0 } else { -EFAULT }
        }
        217 => getdents(p, a[0], a[1], a[2]),
        158 => match a[0] {
            0x1002 => {
                if x86_64::VirtAddr::try_new(a[1]).is_err() {
                    return -EINVAL;
                }
                p.fs_base = a[1];
                super::fs_tab_set(super::thread::key_for(info.pml4), a[1]); // SELFBUILD2: per-thread FS_BASE
                if let Ok(v) = x86_64::VirtAddr::try_new(a[1]) {
                    x86_64::registers::model_specific::FsBase::write(v);
                }
                0
            }
            0x1003 => {
                let fs = super::fs_tab_get(super::thread::key_for(info.pml4)).unwrap_or(p.fs_base); // SELFBUILD2
                if p.asp.copy_out(a[1], &fs.to_le_bytes(), false) { 0 } else { -EFAULT }
            }
            _ => -EINVAL,
        },
        228 => now_ts(p, a[1]),
        273 => 0, // set_robust_list
        318 => {
            // getrandom(buf, len, flags): not cryptographic (xorshift over the clock)
            let n = a[1].min(4096) as usize;
            let mut x = crate::arch::ms() ^ 0x9e37_79b9_7f4a_7c15 ^ (info.pid as u64) << 32;
            let mut v = alloc::vec![0u8; n];
            for b in v.iter_mut() {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                *b = x as u8;
            }
            if p.asp.copy_out(a[0], &v, false) { n as i64 } else { -EFAULT }
        }
        102 | 104 | 107 | 108 | 105 | 106 | 113 | 114 => 0,
        74 | 75 => 0, // fsync / fdatasync: writes are already write-through
        _ => match super::sys2::handle(p, info, nr, a) {
            Some(r) => r,
            None => {
                super::note_enosys(nr);
                -ENOSYS
            }
        },
    }
}
